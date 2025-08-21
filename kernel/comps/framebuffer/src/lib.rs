// SPDX-License-Identifier: MPL-2.0

//! The framebuffer of Asterinas.
#![no_std]
#![deny(unsafe_code)]

extern crate alloc;

mod console;
mod framebuffer;
mod pixel;

use component::{ComponentInitError, init_component};
pub use console::{CONSOLE_NAME, FRAMEBUFFER_CONSOLE, FramebufferConsole};
pub use framebuffer::{FRAMEBUFFER, FrameBuffer};
pub use pixel::{Pixel, PixelFormat, RenderedPixel};

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    framebuffer::init();
    console::init();
    Ok(())
}
