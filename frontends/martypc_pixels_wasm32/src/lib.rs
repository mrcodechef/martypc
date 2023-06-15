#![deny(clippy::all)]
#![forbid(unsafe_code)]

use instant::{Instant, Duration};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

/*
use futures::{
    future::TryFutureExt,
    TryStreamExt
};
*/

use web_sys::{Request, RequestInit, Response, Headers, Blob, FileReader, ProgressEvent, console, window};
use js_sys;

use error_iter::ErrorIter as _;
use log::error;
use pixels::{Pixels, SurfaceTexture};
use std::rc::Rc;
use winit::dpi::LogicalSize;
use winit::event::{Event, VirtualKeyCode};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::WindowBuilder;
use winit_input_helper::WinitInputHelper;

use marty_core::{
    config::{self, *},
    machine::{self, Machine, MachineState, ExecutionControl, ExecutionState},
    cpu_808x::{Cpu, CpuAddress},
    cpu_common::CpuOption,
    rom_manager::{RomManager, RawRomDescriptor},
    floppy_manager::{FloppyManager, FloppyError},
    machine_manager::MACHINE_DESCS,
    vhd_manager::{VHDManager, VHDManagerError},
    vhd::{self, VirtualHardDisk},
    videocard::{RenderMode},
    bytequeue::ByteQueue,
    sound::SoundPlayer,
    syntax_token::SyntaxToken,
    input::{
        self,
        MouseButton
    },
    util
};

use marty_render::{VideoData, VideoRenderer, CompositeParams, ResampleContext};

const DEFAULT_RENDER_WIDTH: u32 = 640;
const DEFAULT_RENDER_HEIGHT: u32 = 400;
const MIN_RENDER_WIDTH: u32 = 160;
const MIN_RENDER_HEIGHT: u32 = 200;
const RENDER_ASPECT: f32 = 0.75;

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const BOX_SIZE: i16 = 64;

pub const FPS_TARGET: f64 = 60.0;
const MICROS_PER_FRAME: f64 = 1.0 / FPS_TARGET * 1000000.0;

/// Representation of the application state. In this example, a box will bounce around the screen.
struct World {
    box_x: i16,
    box_y: i16,
    velocity_x: i16,
    velocity_y: i16,
}

// Rendering Stats
struct Counter {
    frame_count: u64,
    cycle_count: u64,
    instr_count: u64,

    current_ups: u32,
    current_cps: u64,
    current_fps: u32,
    current_ips: u64,
    emulated_fps: u32,
    current_emulated_frames: u64,
    emulated_frames: u64,

    ups: u32,
    fps: u32,
    last_frame: Instant,
    #[allow (dead_code)]
    last_sndbuf: Instant,
    last_second: Instant,
    last_cpu_cycles: u64,
    current_cpu_cps: u64,
    last_system_ticks: u64,
    last_pit_ticks: u64,
    current_sys_tps: u64,
    current_pit_tps: u64,
    emulation_time: Duration,
    render_time: Duration,
    accumulated_us: u128,
    cpu_mhz: f64,
    cycles_per_frame: u32,
    cycle_target: u32,
}

impl Counter {
    fn new() -> Self {
        Self {
            frame_count: 0,
            cycle_count: 0,
            instr_count: 0,
            
            current_ups: 0,
            current_cps: 0,
            current_fps: 0,
            current_ips: 0,

            emulated_fps: 0,
            current_emulated_frames: 0,
            emulated_frames: 0,

            ups: 0,
            fps: 0,
            last_second: Instant::now(),
            last_sndbuf: Instant::now(),
            last_frame: Instant::now(),
            last_cpu_cycles: 0,
            current_cpu_cps: 0,
            last_system_ticks: 0,
            last_pit_ticks: 0,
            current_sys_tps: 0,
            current_pit_tps: 0,
            emulation_time: Duration::ZERO,
            render_time: Duration::ZERO,
            accumulated_us: 0,
            cpu_mhz: 0.0,
            cycles_per_frame: 0,
            cycle_target: 0,
        }
    }
}

#[wasm_bindgen(start)]
fn start() {
    #[cfg(target_arch = "wasm32")]
    {
        std::panic::set_hook(Box::new(console_error_panic_hook::hook));
        console_log::init_with_level(log::Level::Warn).expect("Error initializing logger!");

        wasm_bindgen_futures::spawn_local(run());
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        env_logger::init();
        pollster::block_on(run());
    }
}

pub async fn fetch_binary_file(url: &str) -> Result<Vec<u8>, JsValue> {
    let client_window = window().expect("no global `window` exists");

    let mut opts = RequestInit::new();
    opts.method("GET");

    let request = Request::new_with_str_and_init(url, &opts)?;
    request.headers().set("Content-Type", "application/octet-stream").unwrap();

    let resp_value = JsFuture::from(client_window.fetch_with_request(&request)).await?;
    let resp: Response = resp_value.dyn_into()?;

    let blob = JsFuture::from(resp.blob()?).await?;
    let blob: Blob = blob.dyn_into()?;

    let array_buffer = JsFuture::from(read_blob_as_array_buffer(&blob)).await?;
    let uint8_array = js_sys::Uint8Array::new(&array_buffer);

    let mut vec = vec![0; uint8_array.length() as usize];
    uint8_array.copy_to(&mut vec);

    Ok(vec)
}

fn read_blob_as_array_buffer(blob: &web_sys::Blob) -> js_sys::Promise {
    let file_reader = FileReader::new().unwrap();

    let promise = js_sys::Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let onload = wasm_bindgen::closure::Closure::once(move |event: web_sys::ProgressEvent| {
            let file_reader: FileReader = event.target().unwrap().dyn_into().unwrap();
            let array_buffer = file_reader.result().unwrap();
            resolve.call1(&JsValue::null(), &array_buffer).unwrap();
        });

        file_reader.set_onload(Some(onload.as_ref().unchecked_ref()));
        file_reader.read_as_array_buffer(blob).unwrap();

        onload.forget();
    });

    promise
}

async fn run() {

    // Emulator stuff
    let mut stat_counter = Counter::new();

    let mut video_data = VideoData {
        render_w: DEFAULT_RENDER_WIDTH,
        render_h: DEFAULT_RENDER_HEIGHT,
        aspect_w: 640,
        aspect_h: 480,
        aspect_correction_enabled: false,
        composite_params: Default::default(),
    };

    // Create the video renderer
    let mut video;
    let mut render_src = vec![0; (DEFAULT_RENDER_WIDTH * DEFAULT_RENDER_HEIGHT * 4) as usize];
    // Create resampling context
    let mut resample_context = ResampleContext::new();

    let mut exec_control = ExecutionControl::new();
    exec_control.set_state(ExecutionState::Running);

    // Winit stuff
    let event_loop = EventLoop::new();
    let window = {
        let size = LogicalSize::new(WIDTH as f64, HEIGHT as f64);
        WindowBuilder::new()
            .with_title("Hello Pixels + Web")
            .with_inner_size(size)
            .with_min_inner_size(size)
            .build(&event_loop)
            .expect("WindowBuilder error")
    };

    let window = Rc::new(window);

    let mut machine;

    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use winit::platform::web::WindowExtWebSys;

        // Retrieve current width and height dimensions of browser client window
        let get_window_size = || {
            let client_window = web_sys::window().unwrap();
            LogicalSize::new(
                client_window.inner_width().unwrap().as_f64().unwrap(),
                client_window.inner_height().unwrap().as_f64().unwrap(),
            )
        };

        let window = Rc::clone(&window);

        // Initialize winit window with current dimensions of browser client
        window.set_inner_size(get_window_size());

        let client_window = web_sys::window().unwrap();

        // Attach winit canvas to body element
        web_sys::window()
            .and_then(|win| win.document())
            .and_then(|doc| doc.body())
            .and_then(|body| {
                body.append_child(&web_sys::Element::from(window.canvas()))
                    .ok()
            })
            .expect("Couldn't append canvas to document body!");

        // Try to load toml config.
        let mut opts = web_sys::RequestInit::new();
        opts.method("GET");

        let request = web_sys::Request::new_with_str_and_init("martypc_wasm.toml", &opts).expect("Couldn't create request for configuration file.");
        request.headers().set("Content-Type", "text/plain").expect("Couldn't set headers!");

        let resp_value = JsFuture::from(client_window.fetch_with_request(&request)).await.unwrap();
        let resp: Response = resp_value.into();

        // Get the response as text 
        let toml_text = JsFuture::from(resp.text().unwrap()).await.unwrap();

        // Read config file from toml text
        let mut config = match config::get_config_from_str(&toml_text.as_string().unwrap()){
            Ok(config) => config,
            Err(e) => {
                match e.downcast_ref::<std::io::Error>() {
                    Some(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        log::error!("Configuration file not found!");
                    }
                    Some(e) => {
                        log::error!("Unknown IO error reading configuration file:\n{}", e);
                    }                
                    None => {
                        log::error!("Failed to parse configuration file. There may be a typo or otherwise invalid toml:\n{}", e);
                    }
                }
                return
            }
        };

        video = VideoRenderer::new(config.machine.video);

        let rom_override = match config.machine.rom_override {
            Some(ref rom_override) => rom_override,
            None => panic!("No rom file specified!")
        };

        let floppy_path_str = match config.machine.floppy0 {
            Some(ref floppy) => floppy,
            None => panic!("No floppy image specified!")
        };

        log::warn!("Got config file. Rom to load: {:?} Floppy to load: {:?}", rom_override[0].path, floppy_path_str);
        
        // Convert Path to str
        let rom_path_str = &rom_override[0].path.clone().into_os_string().into_string().unwrap();

        // Get the rom file as a vec<u8>
        let rom_vec = fetch_binary_file(rom_path_str).await.unwrap();

        // Get the floppy image as a vec<u8>
        let floppy_vec = fetch_binary_file(floppy_path_str).await.unwrap();


        //log::warn!("rom: {:?}", rom_vec);

        // Look up the machine description given the machine type in the configuration file
        let machine_desc_opt = MACHINE_DESCS.get(&config.machine.model);
        if let Some(machine_desc) = machine_desc_opt {
            log::warn!("Given machine type {:?} got machine description: {:?}", config.machine.model, machine_desc);
        }
        else {
            log::error!(
                "Couldn't get machine description for machine type {:?}. \
                 Check that you have a valid machine type specified in configuration file.",
                config.machine.model
            );
            return        
        }

        // Init sound 
        // The cpal sound library uses generics to initialize depending on the SampleFormat type.
        // On Windows at least a sample type of f32 is typical, but just in case...
        let sample_fmt = SoundPlayer::get_sample_format();
        let sp = match sample_fmt {
            cpal::SampleFormat::F32 => SoundPlayer::new::<f32>(),
            cpal::SampleFormat::I16 => SoundPlayer::new::<i16>(),
            cpal::SampleFormat::U16 => SoundPlayer::new::<u16>(),
        };

        // Empty features
        let mut features = Vec::new();

        let mut rom_manager = 
            RomManager::new(
                config.machine.model, 
                features,
                config.machine.rom_override.clone(),
            );

        rom_manager.add_raw_rom(
            &rom_vec,
            RawRomDescriptor {
                addr: rom_override[0].address,
                offset: rom_override[0].offset,
                org: rom_override[0].org
            });

        machine = Machine::new(
            &config,
            config.machine.model,
            *machine_desc_opt.unwrap(),
            config.emulator.trace_mode,
            config.machine.video, 
            sp, 
            rom_manager, 
        );

        if let Some(fdc) = machine.fdc() {
            match fdc.load_image_from(0, floppy_vec) {
                Ok(()) => {
                    log::warn!("Floppy image successfully loaded into virtual drive.");
                }
                Err(err) => {
                    log::error!("Floppy image failed to load: {}", err);
                }
            }
        }

        // Listen for resize event on browser client. Adjust winit window dimensions
        // on event trigger
        let closure = wasm_bindgen::closure::Closure::wrap(Box::new(move |_e: web_sys::Event| {
            let size = get_window_size();
            window.set_inner_size(size)
        }) as Box<dyn FnMut(_)>);

        client_window
            .add_event_listener_with_callback("resize", closure.as_ref().unchecked_ref())
            .unwrap();

        closure.forget();
    }

    let mut input = WinitInputHelper::new();
    let mut pixels = {
        let window_size = window.inner_size();
        let surface_texture =
            SurfaceTexture::new(window_size.width, window_size.height, window.as_ref());
        Pixels::new_async(WIDTH, HEIGHT, surface_texture)
            .await
            .expect("Pixels error")
    };
    let mut world = World::new();

    event_loop.run(move |event, _, control_flow| {

        let elapsed_ms = stat_counter.last_second.elapsed().as_millis();
        if elapsed_ms > 1000 {
            log::warn!("FPS: {}", stat_counter.current_fps);
            stat_counter.fps = stat_counter.current_fps;
            stat_counter.current_fps = 0;
            stat_counter.last_second = Instant::now();
        }

        // Decide whether to draw a frame
        let elapsed_us = stat_counter.last_frame.elapsed().as_micros();
        stat_counter.last_frame = Instant::now();

        stat_counter.accumulated_us += elapsed_us;
        
        while stat_counter.accumulated_us > MICROS_PER_FRAME as u128 {

            stat_counter.accumulated_us -= MICROS_PER_FRAME as u128;
            stat_counter.last_frame = Instant::now();
            stat_counter.frame_count += 1;
            stat_counter.current_fps += 1;

            // Emulate a frame worth of instructions
            // ---------------------------------------------------------------------------       

            // Recalculate cycle target based on current CPU speed if it has changed (or uninitialized)
            let mhz = machine.get_cpu_mhz();
            if mhz != stat_counter.cpu_mhz {
                stat_counter.cycles_per_frame = (machine.get_cpu_mhz() * 1000000.0 / FPS_TARGET) as u32;
                stat_counter.cycle_target = stat_counter.cycles_per_frame;
                log::info!("CPU clock has changed to {}Mhz; new cycle target: {}", mhz, stat_counter.cycle_target);
                stat_counter.cpu_mhz = mhz;
            }

            let emulation_start = Instant::now();
            stat_counter.instr_count += machine.run(stat_counter.cycle_target, &mut exec_control);
            stat_counter.emulation_time = Instant::now() - emulation_start;

            // Add instructions to IPS counter
            stat_counter.cycle_count += stat_counter.cycle_target as u64;

                    // Check if there was a resolution change, if a video card is present
                    if let Some(video_card) = machine.videocard() {

                        let new_w;
                        let mut new_h;

                        match video_card.get_render_mode() {
                            RenderMode::Direct => {
                                (new_w, new_h) = video_card.get_display_aperture();

                                // Set a sane maximum
                                if new_h > 240 { 
                                    new_h = 240;
                                }
                            }
                            RenderMode::Indirect => {
                                (new_w, new_h) = video_card.get_display_size();
                            }
                        }

                        // If CGA, we will double scanlines later in the renderer, so make our buffer twice
                        // as high.
                        if video_card.get_scanline_double() {
                            new_h = new_h * 2;
                        }
                        
                        if new_w >= MIN_RENDER_WIDTH && new_h >= MIN_RENDER_HEIGHT {

                            let vertical_delta = (video_data.render_h as i32).wrapping_sub(new_h as i32).abs();

                            // TODO: The vertical delta hack was used for area 8088mph for the old style of rendering.
                            // Now that we render into a fixed frame, we should refactor this
                            if (new_w != video_data.render_w) || ((new_h != video_data.render_h) && (vertical_delta <= 2)) {
                                // Resize buffers
                                log::debug!("Setting internal resolution to ({},{})", new_w, new_h);
                                video_card.write_trace_log(format!("Setting internal resolution to ({},{})", new_w, new_h));

                                // Calculate new aspect ratio (make this option)
                                video_data.render_w = new_w;
                                video_data.render_h = new_h;
                                render_src.resize((new_w * new_h * 4) as usize, 0);                                
                                render_src.fill(0);
    
                                video_data.aspect_w = video_data.render_w;
                                let aspect_corrected_h = f32::floor(video_data.render_w as f32 * RENDER_ASPECT) as u32;
                                // Don't make height smaller
                                let new_height = std::cmp::max(video_data.render_h, aspect_corrected_h);
                                video_data.aspect_h = new_height;
                                
                                // Recalculate sampling factors
                                resample_context.precalc(
                                    video_data.render_w, 
                                    video_data.render_h, 
                                    video_data.aspect_w,
                                    video_data.aspect_h
                                );

                                pixels.frame_mut().fill(0);

                                if let Err(e) = pixels.resize_buffer(video_data.aspect_w, video_data.aspect_h) {
                                    log::error!("Failed to resize pixel pixel buffer: {}", e);
                                }

                                VideoRenderer::set_alpha(pixels.frame_mut(), video_data.aspect_w, video_data.aspect_h, 255);
                            }
                        }
                    }

                    // -- Draw video memory --
                    let composite_enabled = false;
                    let aspect_correct = false;

                    let render_start = Instant::now();

                    // Draw video if there is a video card present
                    let bus = machine.bus_mut();

                    if let Some(video_card) = bus.video() {

                        let beam_pos;
                        let video_buffer;

                        video_buffer = video_card.get_display_buf();
                        beam_pos = None;

                        // Get the render mode from the device and render appropriately
                        match (video_card.get_video_type(), video_card.get_render_mode()) {

                            (VideoType::CGA, RenderMode::Direct) => {
                                // Draw device's front buffer in direct mode (CGA only for now)

                                match aspect_correct {
                                    true => {
                                        video.draw_cga_direct(
                                            &mut render_src,
                                            video_data.render_w, 
                                            video_data.render_h,                                             
                                            video_buffer,
                                            video_card.get_display_extents(),
                                            composite_enabled,
                                            &video_data.composite_params,
                                            beam_pos
                                        );

                                        /*
                                        marty_render::resize_linear(
                                            &render_src, 
                                            video_data.render_w, 
                                            video_data.render_h, 
                                            pixels.frame_mut(), 
                                            video_data.aspect_w, 
                                            video_data.aspect_h,
                                            &resample_context
                                        );
                                        */
                                        marty_render::resize_linear_fast(
                                            &mut render_src, 
                                            video_data.render_w, 
                                            video_data.render_h, 
                                            pixels.frame_mut(), 
                                            video_data.aspect_w, 
                                            video_data.aspect_h,
                                            &mut resample_context
                                        );

                                    }
                                    false => {
                                        video.draw_cga_direct(
                                            pixels.frame_mut(),
                                            video_data.render_w, 
                                            video_data.render_h,                                                                                         
                                            video_buffer,
                                            video_card.get_display_extents(),
                                            composite_enabled,
                                            &video_data.composite_params,
                                            beam_pos                                         
                                        );
                                    }
                                }
                            }
                            (_, RenderMode::Indirect) => {
                                // Draw VRAM in indirect mode
                                match aspect_correct {
                                    true => {
                                        video.draw(&mut render_src, video_card, bus, composite_enabled);
                                        marty_render::resize_linear(
                                            &render_src, 
                                            video_data.render_w, 
                                            video_data.render_h, 
                                            pixels.frame_mut(), 
                                            video_data.aspect_w, 
                                            video_data.aspect_h,
                                            &resample_context
                                        );                            
                                    }
                                    false => {
                                        video.draw(pixels.frame_mut(), video_card, bus, composite_enabled);
                                    }
                                }                                
                            }
                            _ => panic!("Invalid combination of VideoType and RenderMode")
                        }
                    }
        }

        // Draw the current frame
        if let Event::RedrawRequested(_) = event {
            
            //world.draw(pixels.frame_mut());

            //stat_counter.current_fps += 1;

            if let Err(err) = pixels.render() {
                log_error("pixels.render", err);
                *control_flow = ControlFlow::Exit;
                return;
            }
        }

        // Handle input events
        if input.update(&event) {
            // Close events
            if input.key_pressed(VirtualKeyCode::Escape) || input.quit() {
                *control_flow = ControlFlow::Exit;
                return;
            }

            // Resize the window
            if let Some(size) = input.window_resized() {
                if let Err(err) = pixels.resize_surface(size.width, size.height) {
                    log_error("pixels.resize_surface", err);
                    *control_flow = ControlFlow::Exit;
                    return;
                }
            }

            // Update internal state and request a redraw
            //world.update();

            window.request_redraw();
        }
    });
}

fn log_error<E: std::error::Error + 'static>(method_name: &str, err: E) {
    error!("{method_name}() failed: {err}");
    for source in err.sources().skip(1) {
        error!("  Caused by: {source}");
    }
}

impl World {
    /// Create a new `World` instance that can draw a moving box.
    fn new() -> Self {
        Self {
            box_x: 24,
            box_y: 16,
            velocity_x: 1,
            velocity_y: 1,
        }
    }

    /// Update the `World` internal state; bounce the box around the screen.
    fn update(&mut self) {
        if self.box_x <= 0 || self.box_x + BOX_SIZE > WIDTH as i16 {
            self.velocity_x *= -1;
        }
        if self.box_y <= 0 || self.box_y + BOX_SIZE > HEIGHT as i16 {
            self.velocity_y *= -1;
        }

        self.box_x += self.velocity_x;
        self.box_y += self.velocity_y;
    }

    /// Draw the `World` state to the frame buffer.
    ///
    /// Assumes the default texture format: `wgpu::TextureFormat::Rgba8UnormSrgb`
    fn draw(&self, frame: &mut [u8]) {
        for (i, pixel) in frame.chunks_exact_mut(4).enumerate() {
            let x = (i % WIDTH as usize) as i16;
            let y = (i / WIDTH as usize) as i16;

            let inside_the_box = x >= self.box_x
                && x < self.box_x + BOX_SIZE
                && y >= self.box_y
                && y < self.box_y + BOX_SIZE;

            let rgba = if inside_the_box {
                [0x5e, 0x48, 0xe8, 0xff]
            } else {
                [0x48, 0xb2, 0xe8, 0xff]
            };

            pixel.copy_from_slice(&rgba);
        }
    }
}