use crate::error::{AvError, Error};
use crate::sys;
use core::num;
use std::ffi::{CString, c_void};

#[derive(Debug, Clone, PartialEq)]
enum OptValue {
    Int(i64),
    Double(f64),
    Str(String),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilterOptions {
    entries: Vec<(String, OptValue)>,
}

impl FilterOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn int(mut self, name: &str, v: i64) -> Self {
        self.entries.push((name.to_owned(), OptValue::Int(v)));
        self
    }
    pub fn double(mut self, name: &str, v: f64) -> Self {
        self.entries.push((name.to_owned(), OptValue::Double(v)));
        self
    }
    pub fn string(mut self, name: &str, v: impl Into<String>) -> Self {
        self.entries.push((name.to_owned(), OptValue::Str(v.into())));
        self
    }
    pub fn rational(self, name: &str, num: i32, den: i32) -> Self {
        self.string(name, format!("{num}/{den}"))
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(n, _)| n.as_str())
    }

    /// # Safety
    /// `ctx` must be an allocated, not-yet-initialized AVFilterContext.
    pub(crate) unsafe fn apply(&self, ctx: *mut sys::AVFilterContext) -> Result<(), Error> {
        let obj = ctx.cast::<c_void>();
        let flags = sys::AV_OPT_SEARCH_CHILDREN as i32;
        for (name, value) in &self.entries {
            let cname = CString::new(name.as_str())?;
            let ret = match value {
                OptValue::Int(v) => unsafe { sys::av_opt_set_int(obj, cname.as_ptr(), *v, flags) },
                OptValue::Double(v) => unsafe { sys::av_opt_set_double(obj, cname.as_ptr(), *v, flags) },
                OptValue::Str(v) => {
                    let cv = CString::new(v.as_str())?;
                    unsafe { sys::av_opt_set(obj, cname.as_ptr(), cv.as_ptr(), flags) }
                }
            };
            if ret < 0 {
                return Err(Error::Option { name: name.clone(), code: AvError(ret) });
            }
        }
        Ok(())
    }
}
