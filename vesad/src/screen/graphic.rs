use std::collections::VecDeque;
use std::convert::TryInto;
use std::{mem, slice};

use orbclient::{Event, ResizeEvent};
use syscall::error::*;

use crate::display::Display;
use crate::screen::Screen;

// Keep synced with orbital
#[derive(Clone, Copy)]
#[repr(packed)]
struct SyncRect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

pub struct GraphicScreen {
    pub display: Display,
    pub input: VecDeque<Event>,
    sync_rects: Vec<SyncRect>,
}

impl GraphicScreen {
    pub fn new(display: Display) -> GraphicScreen {
        GraphicScreen {
            display: display,
            input: VecDeque::new(),
            sync_rects: Vec::new(),
        }
    }
}

impl Screen for GraphicScreen {
    fn width(&self) -> usize {
        self.display.width
    }

    fn height(&self) -> usize {
        self.display.height
    }

    fn resize(&mut self, width: usize, height: usize) {
        //TODO: Fix issue with mapped screens
        self.display.resize(width, height);
        self.input.push_back(ResizeEvent {
            width: width as u32,
            height: height as u32,
        }.to_event());
    }

    fn map(&self, offset: usize, size: usize) -> Result<usize> {
        if offset + size <= self.display.offscreen.len() * 4 {
            Ok(self.display.offscreen.as_ptr() as usize + offset)
        } else {
            Err(Error::new(EINVAL))
        }
    }

    fn input(&mut self, event: &Event) {
        self.input.push_back(*event);
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut i = 0;

        let event_buf = unsafe { slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut Event, buf.len()/mem::size_of::<Event>()) };

        while i < event_buf.len() && ! self.input.is_empty() {
            event_buf[i] = self.input.pop_front().unwrap();
            i += 1;
        }

        Ok(i * mem::size_of::<Event>())
    }

    fn can_read(&self) -> Option<usize> {
        if self.input.is_empty() {
            None
        } else {
            Some(self.input.len() * mem::size_of::<Event>())
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        let sync_rects = unsafe {
            slice::from_raw_parts(
                buf.as_ptr() as *const SyncRect,
                buf.len() / mem::size_of::<SyncRect>()
            )
        };

        self.sync_rects.extend_from_slice(sync_rects);

        Ok(sync_rects.len() * mem::size_of::<SyncRect>())
    }

    fn seek(&mut self, _pos: isize, _whence: usize) -> Result<usize> {
        Ok(0)
    }

    fn sync(&mut self, onscreen: &mut [u32], stride: usize) {
        for sync_rect in self.sync_rects.drain(..) {
            self.display.sync(
                sync_rect.x.try_into().unwrap_or(0),
                sync_rect.y.try_into().unwrap_or(0),
                sync_rect.w.try_into().unwrap_or(0),
                sync_rect.h.try_into().unwrap_or(0),
                onscreen,
                stride,
            );
        }
    }

    fn redraw(&mut self, onscreen: &mut [u32], stride: usize) {
        let width = self.display.width;
        let height = self.display.height;
        self.display.sync(0, 0, width, height, onscreen, stride);
    }
}
