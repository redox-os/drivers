extern crate ransid;

use std::collections::VecDeque;

use syscall::error::*;

use crate::display::Display;

pub struct TextScreen {
    pub display: Display,
    inner: console_draw::TextScreen,
}

impl TextScreen {
    pub fn new(display: Display) -> TextScreen {
        TextScreen {
            display,
            inner: console_draw::TextScreen::new(),
        }
    }
}

impl TextScreen {
    pub fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let mut map = self.display.map.lock().unwrap();
        let damage = self.inner.write(
            &mut console_draw::DisplayMap {
                offscreen: map.inner.ptr_mut(),
                width: map.inner.width(),
                height: map.inner.height(),
            },
            buf,
            &mut VecDeque::new(),
        );
        drop(map);

        self.display.sync_rects(damage);

        Ok(buf.len())
    }
}
