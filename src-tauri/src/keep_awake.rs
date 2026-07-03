//! 阻止 macOS 系统空闲休眠（允许屏幕熄灭，允许合盖休眠）。
//!
//! 使用 macOS IOKit 的 `IOPMAssertionCreateWithName` API 创建
//! `PreventUserIdleSystemSleep` 类型的 assertion：
//! - 系统不会因用户空闲而自动休眠（后台 Claude Code 进程持续运行）
//! - 屏幕可以正常熄灭（省电）
//! - 合盖仍会休眠（硬件行为，无法阻止也不应阻止）
//!
//! 手写 FFI 而非用 `nosleep` crate：仅需 4 个 C 函数，零额外依赖。

// ── macOS 实现 ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Mutex;

    /// 全局 assertion ID；None 表示未激活。
    static ASSERTION: Mutex<Option<u32>> = Mutex::new(None);

    // ── FFI 声明 ──────────────────────────────────────────────────────

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: *mut std::ffi::c_void, // kCFAllocatorDefault = null
            c_str: *const u8,
            encoding: u32, // kCFStringEncodingUTF8 = 0x08000100
        ) -> *mut std::ffi::c_void;

        fn CFRelease(cf: *mut std::ffi::c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        /// kIOPMAssertionLevelOn = 255
        fn IOPMAssertionCreateWithName(
            assertion_type: *mut std::ffi::c_void, // CFStringRef
            level: u32,
            name: *mut std::ffi::c_void, // CFStringRef
            assertion_id: *mut u32,
        ) -> i32; // IOReturn (0 = kIOReturnSuccess)

        fn IOPMAssertionRelease(assertion_id: u32) -> i32;
    }

    // kCFStringEncodingUTF8
    const UTF8_ENCODING: u32 = 0x0800_0100;
    // kIOPMAssertionLevelOn
    const LEVEL_ON: u32 = 255;
    // kIOReturnSuccess
    const SUCCESS: i32 = 0;

    /// RAII 包装 CFString，Drop 时自动释放。
    struct CfString(*mut std::ffi::c_void);

    impl CfString {
        fn new(s: &str) -> Option<Self> {
            let ptr = unsafe {
                CFStringCreateWithCString(
                    std::ptr::null_mut(),
                    s.as_ptr(),
                    UTF8_ENCODING,
                )
            };
            if ptr.is_null() {
                None
            } else {
                Some(CfString(ptr))
            }
        }

        fn as_ptr(&self) -> *mut std::ffi::c_void {
            self.0
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    // ── 公开 API ──────────────────────────────────────────────────────

    /// 创建 PreventUserIdleSystemSleep assertion，阻止系统空闲休眠。
    /// 屏幕可正常熄灭，合盖仍会休眠。
    pub fn prevent_idle_sleep() {
        let mut guard = match ASSERTION.lock() {
            Ok(g) => g,
            Err(e) => {
                log::error!("[keep-awake] lock poisoned: {e}");
                return;
            }
        };

        // 已有 assertion，跳过
        if guard.is_some() {
            log::debug!("[keep-awake] assertion already active, skipping");
            return;
        }

        let type_str = CfString::new("PreventUserIdleSystemSleep");
        let name_str = CfString::new("OneCode Desktop is running");

        match (type_str, name_str) {
            (Some(t), Some(n)) => {
                let mut assertion_id: u32 = 0;
                let rc = unsafe {
                    IOPMAssertionCreateWithName(
                        t.as_ptr(),
                        LEVEL_ON,
                        n.as_ptr(),
                        &mut assertion_id,
                    )
                };
                if rc == SUCCESS {
                    log::info!("[keep-awake] assertion created (id={assertion_id}), system will not idle-sleep");
                    *guard = Some(assertion_id);
                } else {
                    log::error!("[keep-awake] IOPMAssertionCreateWithName failed: rc={rc}");
                }
            }
            _ => {
                log::error!("[keep-awake] CFString creation failed (out of memory?)");
            }
        }
    }

    /// 释放 assertion，恢复系统正常休眠行为。
    pub fn allow_idle_sleep() {
        let mut guard = match ASSERTION.lock() {
            Ok(g) => g,
            Err(e) => {
                log::error!("[keep-awake] lock poisoned on release: {e}");
                return;
            }
        };

        if let Some(id) = guard.take() {
            let rc = unsafe { IOPMAssertionRelease(id) };
            if rc == SUCCESS {
                log::info!("[keep-awake] assertion released (id={id}), system can idle-sleep again");
            } else {
                log::warn!("[keep-awake] IOPMAssertionRelease failed: rc={rc} (assertion will clear on process exit)");
            }
        }
    }
}

// ── 非 macOS 平台的 no-op 实现 ──────────────────────────────────────────

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn prevent_idle_sleep() {
        log::info!("[keep-awake] not implemented on this platform (no-op)");
    }
    pub fn allow_idle_sleep() {
        // no-op
    }
}

// ── 统一公开接口 ──────────────────────────────────────────────────────

pub use imp::{allow_idle_sleep, prevent_idle_sleep};
