use driver_graphics::GraphicsAdapter;
use graphics_ipc::legacy::Damage;

use crate::{framebuffer::FrameBuffer, screen::GraphicScreen};

pub struct FbAdapter {
    pub framebuffers: Vec<FrameBuffer>,
}

impl GraphicsAdapter for FbAdapter {
    type Resource = GraphicScreen;

    fn displays(&self) -> Vec<usize> {
        (0..self.framebuffers.len()).collect()
    }

    fn display_size(&self, display_id: usize) -> (u32, u32) {
        (
            self.framebuffers[display_id].width as u32,
            self.framebuffers[display_id].height as u32,
        )
    }

    fn create_resource(&mut self, width: u32, height: u32) -> Self::Resource {
        GraphicScreen::new(width as usize, height as usize)
    }

    fn map_resource(&mut self, resource: &Self::Resource) -> *mut u8 {
        resource.ptr()
    }

    fn set_scanout(&mut self, display_id: usize, resource: &Self::Resource) {
        resource.redraw(&mut self.framebuffers[display_id]);
    }

    fn flush_resource(
        &mut self,
        display_id: usize,
        resource: &Self::Resource,
        damage: Option<&[Damage]>,
    ) {
        if let Some(damage) = damage {
            resource.sync(&mut self.framebuffers[display_id], damage)
        } else {
            resource.redraw(&mut self.framebuffers[display_id])
        }
    }
}
