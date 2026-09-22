use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};

pub(super) struct Assertion {
    id: u32,
}

impl Assertion {
    pub(super) fn new(name: &str) -> anyhow::Result<Self> {
        let assertion_type = CFString::new("PreventUserIdleSystemSleep");
        let name = CFString::new(name);
        let mut id = 0;
        let result = unsafe {
            IOPMAssertionCreateWithName(
                assertion_type.as_concrete_TypeRef(),
                255,
                name.as_concrete_TypeRef(),
                &mut id,
            )
        };
        match result {
            0 => Ok(Self { id }),
            status => Err(anyhow::anyhow!(
                "Could not prevent idle system sleep: IOKit status {status:#x}"
            )),
        }
    }
}

impl Drop for Assertion {
    fn drop(&mut self) {
        unsafe {
            IOPMAssertionRelease(self.id);
        }
    }
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMAssertionCreateWithName(
        assertion_type: CFStringRef,
        level: u32,
        name: CFStringRef,
        id: *mut u32,
    ) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}
