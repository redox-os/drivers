use std::alloc::{self, Layout};
use std::ptr;
use std::ptr::NonNull;

pub struct OffscreenBuffer {
    ptr: NonNull<[u32]>,
}

impl OffscreenBuffer {
    #[inline]
    fn layout(len: usize) -> Layout {
        // optimizes to an integer mul
        Layout::array::<u32>(len).unwrap().align_to(4096).unwrap()
    }

    #[inline]
    pub fn new(len: usize) -> Self {
        let layout = Self::layout(len);
        let ptr = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = ptr::slice_from_raw_parts_mut(ptr.cast(), len);
        let ptr = NonNull::new(ptr).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        OffscreenBuffer { ptr }
    }
}
impl Drop for OffscreenBuffer {
    fn drop(&mut self) {
        let layout = Self::layout(self.ptr.len());
        unsafe { alloc::dealloc(self.ptr.as_ptr().cast(), layout) };
    }
}
impl std::ops::Deref for OffscreenBuffer {
    type Target = [u32];
    fn deref(&self) -> &[u32] {
        unsafe { self.ptr.as_ref() }
    }
}
impl std::ops::DerefMut for OffscreenBuffer {
    fn deref_mut(&mut self) -> &mut [u32] {
        unsafe { self.ptr.as_mut() }
    }
}
