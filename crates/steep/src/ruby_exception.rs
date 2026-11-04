use std::{
    fmt,
    panic::{self, AssertUnwindSafe},
};

use rbs_definition::errors::RbsError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RubyException {
    pub class_name: String,
    pub message: String,
}

impl RubyException {
    pub fn new(class_name: impl Into<String>, message: impl Into<String>) -> RubyException {
        RubyException {
            class_name: class_name.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for RubyException {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}({})", self.message, self.class_name)
    }
}

impl From<String> for RubyException {
    fn from(message: String) -> RubyException {
        let class_name = if message.starts_with("undefined method ") {
            "NoMethodError"
        } else if message.starts_with("key not found: ") {
            "KeyError"
        } else {
            "RuntimeError"
        };
        RubyException::new(class_name, message)
    }
}

impl From<RbsError> for RubyException {
    fn from(error: RbsError) -> RubyException {
        RubyException::new(error.class_name(), error.to_string())
    }
}

pub fn raise(exception: impl Into<RubyException>) -> ! {
    panic::panic_any(exception.into())
}

pub fn raise_runtime_error(message: impl Into<String>) -> ! {
    raise(RubyException::new("RuntimeError", message))
}

pub fn fetch<Item>(items: &[Item], index: usize) -> &Item {
    items.get(index).unwrap_or_else(|| {
        let length = items.len();
        let lower_bound = if length == 0 {
            "0".to_string()
        } else {
            format!("-{length}")
        };
        raise(RubyException::new(
            "IndexError",
            format!("index {index} outside of array bounds: {lower_bound}...{length}"),
        ))
    })
}

pub fn rescue<Output>(block: impl FnOnce() -> Output) -> Result<Output, RubyException> {
    panic::catch_unwind(AssertUnwindSafe(block)).map_err(|payload| {
        match payload.downcast::<RubyException>() {
            Ok(exception) => *exception,
            Err(payload) => panic::resume_unwind(payload),
        }
    })
}

pub fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        if !information.payload().is::<RubyException>() {
            default_hook(information);
        }
    }));
}
