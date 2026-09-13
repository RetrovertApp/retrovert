//! Can this box start Vulkan the way Qt starts it?
//!
//! Quickshell hands `QRhi::create` a `QVulkanInstance` it never created when the loader is
//! present but cannot deliver, and dies with SIGSEGV instead of raising the scene-graph error
//! QML can listen for. So the launcher asks the loader the same three things Qt will, for
//! about 8 ms, and starts on OpenGL when any of them refuses.
//!
//! Ported from Flea's `src/vulkan.rs`, copyright (c) 2026 GM, MIT licensed.

use std::ffi::{c_void, CStr, OsStr};
use std::os::raw::{c_char, c_int};
use std::ptr::{null, null_mut};

const RTLD_NOW: c_int = 2;
const LIBVULKAN: &CStr = c"libvulkan.so.1";
const VK_SUCCESS: i32 = 0;
const INSTANCE_CREATE_INFO: u32 = 1;

#[repr(C)]
struct InstanceCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    p_application_info: *const c_void,
    enabled_layer_count: u32,
    pp_enabled_layer_names: *const *const c_char,
    enabled_extension_count: u32,
    pp_enabled_extension_names: *const *const c_char,
}

extern "C" {
    fn dlopen(file: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *mut c_char;
}

type CreateInstance =
    unsafe extern "C" fn(*const InstanceCreateInfo, *const c_void, *mut *mut c_void) -> i32;
type EnumeratePhysicalDevices =
    unsafe extern "C" fn(*mut c_void, *mut u32, *mut *mut c_void) -> i32;
type DestroyInstance = unsafe extern "C" fn(*mut c_void, *const c_void);

const SURFACE: &CStr = c"VK_KHR_surface";
const WAYLAND_SURFACE: &CStr = c"VK_KHR_wayland_surface";
const XCB_SURFACE: &CStr = c"VK_KHR_xcb_surface";

fn platform_surface() -> &'static CStr {
    surface_for(std::env::var_os("WAYLAND_DISPLAY").as_deref())
}

/// An exported-but-empty `WAYLAND_DISPLAY` is not a Wayland session.
fn surface_for(wayland_display: Option<&OsStr>) -> &'static CStr {
    match wayland_display {
        Some(value) if !value.is_empty() => WAYLAND_SURFACE,
        _ => XCB_SURFACE,
    }
}

unsafe fn dl_reason() -> String {
    // SAFETY: dlerror returns null or a string the loader owns until the next dl call.
    let text = unsafe { dlerror() };
    if text.is_null() {
        return String::from("the dynamic loader gave no reason");
    }
    // SAFETY: non-null dlerror text is NUL-terminated.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

unsafe fn entry(library: *mut c_void, symbol: &CStr) -> Result<*mut c_void, String> {
    // SAFETY: library came from a successful dlopen and symbol is NUL-terminated.
    let found = unsafe { dlsym(library, symbol.as_ptr()) };
    if found.is_null() {
        // SAFETY: reading dlerror right after the failed dlsym.
        let reason = unsafe { dl_reason() };
        return Err(format!(
            "libvulkan.so.1 has no {}, {reason}",
            symbol.to_string_lossy()
        ));
    }
    Ok(found)
}

/// Ok when Vulkan can start the way Qt starts it; the error is the sentence the operator reads.
pub fn usable() -> Result<(), String> {
    usable_with(&[SURFACE, platform_surface()])
}

// The handle is never dlclose()d: this process execs qs a moment later.
fn usable_with(extensions: &[&CStr]) -> Result<(), String> {
    let names: Vec<*const c_char> = extensions.iter().map(|e| e.as_ptr()).collect();
    let asked = extensions
        .iter()
        .map(|e| e.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" and ");
    // SAFETY: every pointer handed to the loader is a live NUL-terminated string or a struct
    // laid out as the Vulkan ABI declares it; the function pointers are transmuted from the
    // symbols Vulkan documents under those exact names and signatures.
    unsafe {
        let library = dlopen(LIBVULKAN.as_ptr(), RTLD_NOW);
        if library.is_null() {
            return Err(format!("libvulkan.so.1 did not load, {}", dl_reason()));
        }
        let create: CreateInstance = std::mem::transmute(entry(library, c"vkCreateInstance")?);
        let enumerate: EnumeratePhysicalDevices =
            std::mem::transmute(entry(library, c"vkEnumeratePhysicalDevices")?);
        let destroy: DestroyInstance = std::mem::transmute(entry(library, c"vkDestroyInstance")?);

        let request = InstanceCreateInfo {
            s_type: INSTANCE_CREATE_INFO,
            p_next: null(),
            flags: 0,
            p_application_info: null(),
            enabled_layer_count: 0,
            pp_enabled_layer_names: null(),
            enabled_extension_count: u32::try_from(names.len()).unwrap_or(0),
            pp_enabled_extension_names: names.as_ptr(),
        };
        let mut instance: *mut c_void = null_mut();
        let created = create(&raw const request, null(), &raw mut instance);
        if created != VK_SUCCESS {
            return Err(format!("vkCreateInstance answered {created} for {asked}"));
        }
        if instance.is_null() {
            return Err(format!(
                "vkCreateInstance took {asked} and returned no instance"
            ));
        }
        let mut devices: u32 = 0;
        let listed = enumerate(instance, &raw mut devices, null_mut());
        destroy(instance, null());
        if listed != VK_SUCCESS {
            return Err(format!("vkEnumeratePhysicalDevices answered {listed}"));
        }
        if devices == 0 {
            return Err(String::from(
                "vkEnumeratePhysicalDevices succeeded and listed no device",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_required_extension_no_loader_offers_reads_unusable() {
        let absent = c"VK_KHR_retrovert_probe_extension_that_cannot_exist";
        let reason = usable_with(&[SURFACE, absent]).unwrap_err();
        assert!(reason.starts_with("vkCreateInstance answered"), "{reason}");
        assert!(reason.contains("VK_KHR_surface"), "{reason}");
    }

    #[test]
    fn the_surface_extension_follows_the_session() {
        assert_eq!(surface_for(Some(OsStr::new("wayland-1"))), WAYLAND_SURFACE);
        assert_eq!(surface_for(Some(OsStr::new(""))), XCB_SURFACE);
        assert_eq!(surface_for(None), XCB_SURFACE);
    }
}
