/*
    MartyPC Emulator 
    (C)2023 Daniel Balsom
    https://github.com/dbalsom/marty

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with this program.  If not, see <https://www.gnu.org/licenses/>.

    --------------------------------------------------------------------------

    devices::mouse.rs

    Implements a Microsoft Serial Mouse
 
 */
use std::{
    collections::VecDeque
};

use crate::devices::serial::SerialPortController;

// Scale factor for real vs emulated mouse deltas. Need to play with
// this value until it feels right.
const MOUSE_SCALE: f64 = 0.25;

// Mouse port is always attached to COM1
const MOUSE_PORT: usize = 0;

// Microseconds with RTS low before mouse considers itself reset
const MOUSE_RESET_TIME: f64 = 10_000.0;

// Mouse sends this byte when RTS is held low for MOUSE_RESET_TIME
// 0x4D = Ascii 'M' (For 'Microsoft' perhaps?)
const MOUSE_RESET_ACK_BYTE: u8 = 0x4D;

const MOUSE_UPDATE_STARTBIT: u8 = 0b0100_0000;
const MOUSE_UPDATE_LBUTTON: u8 = 0b0010_0000;
const MOUSE_UPDATE_RBUTTON: u8 = 0b0001_0000;
const MOUSE_UPDATE_HO_BITS: u8 = 0b1100_0000;
const MOUSE_UPDATE_LO_BITS: u8 = 0b0011_1111;

#[allow(dead_code)]
pub struct Mouse {

    updates: VecDeque<MouseUpdate>,
    rts: bool,
    rts_low_timer: f64,
    dtr: bool,
}

pub enum MouseUpdate {
    Update(u8, u8, u8)
}

impl Mouse {
    pub fn new() -> Self {
        Self {
            updates: VecDeque::new(),
            rts: false,
            rts_low_timer: 0.0,
            dtr: false,
        }
    }

    pub fn update(&mut self, l_button_pressed: bool, r_button_pressed: bool, delta_x: f64, delta_y: f64) {

        let mut scaled_x = delta_x * MOUSE_SCALE;
        let mut scaled_y = delta_y * MOUSE_SCALE;
    
        // Mouse scale can cause fractional integer updates. Adjust to Minimum movement of one unit
        if scaled_x > 0.0 && scaled_x < 1.0 {
            scaled_x = 1.0;
        }
        if scaled_x < 0.0 && scaled_x > -1.0 {
            scaled_x = -1.0;
        }
        if scaled_y > 0.0 && scaled_y < 1.0 {
            scaled_y = 1.0;
        }
        if scaled_y < 0.0 && scaled_y > -1.0 {
            scaled_y = -1.0;
        }        
        let delta_x_i8 = scaled_x as i8;
        let delta_y_i8 = scaled_y as i8;

        let mut byte1 = MOUSE_UPDATE_STARTBIT;

        if l_button_pressed {
            //log::debug!("Sending mouse button down");
            byte1 |= MOUSE_UPDATE_LBUTTON;
        }
        /*
        else {
            log::debug!("Sending mouse button up");
        }
        */

        if r_button_pressed {
            byte1 |= MOUSE_UPDATE_RBUTTON;
        }

        // Pack HO 2 bits of Y into byte1
        byte1 |= ((delta_y_i8 as u8) & MOUSE_UPDATE_HO_BITS) >> 4;
        // Pack HO 2 bits of X into byte1;
        byte1 |= ((delta_x_i8 as u8) & MOUSE_UPDATE_HO_BITS) >> 6;

        // LO 6 bits of X into byte 2
        let byte2 = (delta_x_i8 as u8) & MOUSE_UPDATE_LO_BITS;
        // LO 6 bits of Y into byte 3
        let byte3 = (delta_y_i8 as u8) & MOUSE_UPDATE_LO_BITS;

        // Queue update

        self.updates.push_back(MouseUpdate::Update(byte1, byte2, byte3));
        /*
        let mut serial = self.serial_ctrl.borrow_mut();
        serial.queue_byte(MOUSE_PORT, byte1);
        serial.queue_byte(MOUSE_PORT, byte2);
        serial.queue_byte(MOUSE_PORT, byte3);*/


     }

    /// Run the mouse device for the specified number of microseconds
    pub fn run(&mut self, serial: &mut SerialPortController, us: f64) {

        // Send a queued update.
        if let Some(MouseUpdate::Update(byte1, byte2, byte3)) = self.updates.pop_front() {
            serial.queue_byte(MOUSE_PORT, byte1);
            serial.queue_byte(MOUSE_PORT, byte2);
            serial.queue_byte(MOUSE_PORT, byte3);
        }

        // Check RTS line for mouse reset
        let rts = serial.get_rts(MOUSE_PORT);

        if self.rts && !rts {
            // RTS has gone low
            self.rts = false;
            self.rts_low_timer = 0.0;
        }
        else if !self.rts && !rts {
            // RTS remains low, count
            self.rts_low_timer += us;
        }
        else if rts && !self.rts {
            // RTS has gone high

            self.rts = true;

            if self.rts_low_timer > MOUSE_RESET_TIME {
                // Reset mouse
                self.rts_low_timer = 0.0;
                // Send reset ack byte
                log::trace!("Sending reset byte: {:02X}", MOUSE_RESET_ACK_BYTE );
                serial.queue_byte(MOUSE_PORT, MOUSE_RESET_ACK_BYTE);
            }
        }
    }
}

