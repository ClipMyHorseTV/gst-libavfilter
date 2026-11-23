use crate::sys;
use std::ffi::{CStr, CString, c_void};
use std::ptr;

#[derive(Clone, Copy)]
pub struct FilterInfo(&'static sys::AVFilter);

impl FilterInfo {
    pub fn lookup(name: &str) -> Option<Self> {
        let cname = CString::new(name).ok()?;
        unsafe { sys::avfilter_get_by_name(cname.as_ptr()).as_ref() }.map(FilterInfo)
    }

    pub fn description(&self) -> String {
        if self.0.description.is_null() {
            return String::new();
        }
        unsafe { CStr::from_ptr(self.0.description) }.to_string_lossy().into_owned()
    }

    /// Does this filter have an option with this name?
    pub fn has_option(&self, name: &str) -> bool {
        let class = self.0.priv_class;
        let Ok(cname) = CString::new(name) else { return false };
        if class.is_null() {
            return false;
        };
        // With AV_OPT_SEARCH_FAKE_OBJ, `obj` must point to an `AVClass*` (like the first field
        // of a real object), not to the AVClass itself.
        let found = unsafe {
            sys::av_opt_find(
                &class as *const *const sys::AVClass as *mut c_void,
                cname.as_ptr(),
                ptr::null(),
                0,
                sys::AV_OPT_SEARCH_FAKE_OBJ as i32,
            )
        };
        !found.is_null()
    }
}

/// True if the linked libavfilter was built with --enable-gpl.
pub fn library_is_gpl() -> bool {
    let lic = unsafe { CStr::from_ptr(sys::avfilter_license()) };
    lic.to_bytes().starts_with(b"GPL")
}
