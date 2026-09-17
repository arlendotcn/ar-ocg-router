//! Service integration.
//!
//! Windows: a real service (implements the SCM protocol through advapi32, no extra crates),
//! so it can be installed/started/stopped by the Service Control Manager:
//!     ar-ocg-router.exe --install-service --config C:\\path\\config.yaml
//!     ar-ocg-router.exe --service            (what the SCM runs)
//!     ar-ocg-router.exe --uninstall-service
//!
//! Linux: use the systemd unit shipped in dist (scripts/ar-ocg-router.service);
//! this module only provides the shared shutdown flag.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DEFAULT_SERVICE_NAME: &str = "ar-ocg-router";
pub const PRODUCT_NAME: &str = "ar-OCG-Router";

/// Set when the SCM (or any other supervisor) asks the process to stop.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

pub fn is_shutdown() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

/// Default log file used when running with no console (service mode).
pub fn default_log_path() -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("ar-ocg-router.log")
}

pub fn description(config: &Path) -> String {
    let shown = config.display().to_string();
    let shown = shown
        .strip_prefix("\\\\?\\")
        .map(|s| s.to_string())
        .unwrap_or(shown);
    format!(
        "{} - multi-account LLM router (OpenCode Go + DeepSeek official). Config: {}",
        PRODUCT_NAME, shown
    )
}

// --------------------------------------------------------------------------- windows

#[cfg(windows)]
mod platform {
    use super::{description, DEFAULT_SERVICE_NAME, PRODUCT_NAME};
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
    use std::sync::OnceLock;

    type Handle = *mut core::ffi::c_void;
    type Dword = u32;
    type WinBool = i32;

    const SC_MANAGER_CONNECT: Dword = 0x0001;
    const SC_MANAGER_CREATE_SERVICE: Dword = 0x0002;

    const SERVICE_QUERY_STATUS: Dword = 0x0004;
    const SERVICE_START: Dword = 0x0010;
    const SERVICE_STOP: Dword = 0x0020;
    const SERVICE_CHANGE_CONFIG: Dword = 0x0002;
    const DELETE: Dword = 0x00010000;

    const SERVICE_WIN32_OWN_PROCESS: Dword = 0x0000_0010;
    const SERVICE_AUTO_START: Dword = 0x0000_0002;
    const SERVICE_ERROR_NORMAL: Dword = 0x0000_0001;

    const SERVICE_CONTROL_STOP: Dword = 0x0000_0001;
    const SERVICE_CONTROL_INTERROGATE: Dword = 0x0000_0004;
    const SERVICE_CONTROL_SHUTDOWN: Dword = 0x0000_0005;

    const SERVICE_ACCEPT_STOP: Dword = 0x0000_0001;
    const SERVICE_ACCEPT_SHUTDOWN: Dword = 0x0000_0004;

    const SERVICE_STOPPED: Dword = 0x0000_0001;
    const SERVICE_START_PENDING: Dword = 0x0000_0002;
    const SERVICE_STOP_PENDING: Dword = 0x0000_0003;
    const SERVICE_RUNNING: Dword = 0x0000_0004;

    const SERVICE_CONFIG_DESCRIPTION: Dword = 1;
    const SERVICE_CONFIG_FAILURE_ACTIONS: Dword = 2;
    const SC_ACTION_RESTART: Dword = 1;

    #[repr(C)]
    struct ServiceStatus {
        dw_service_type: Dword,
        dw_current_state: Dword,
        dw_controls_accepted: Dword,
        dw_win32_exit_code: Dword,
        dw_service_specific_exit_code: Dword,
        dw_check_point: Dword,
        dw_wait_hint: Dword,
    }

    #[repr(C)]
    struct ServiceStatusProcess {
        dw_service_type: Dword,
        dw_current_state: Dword,
        dw_controls_accepted: Dword,
        dw_win32_exit_code: Dword,
        dw_service_specific_exit_code: Dword,
        dw_check_point: Dword,
        dw_wait_hint: Dword,
        dw_process_id: Dword,
        dw_service_flags: Dword,
    }

    #[repr(C)]
    struct ServiceTableEntryW {
        lp_service_name: *const u16,
        lp_service_proc: Option<extern "system" fn(Dword, *mut *mut u16)>,
    }

    #[repr(C)]
    struct ServiceDescriptionW {
        lp_description: *mut u16,
    }

    #[repr(C)]
    struct ScAction {
        kind: Dword,
        delay_ms: Dword,
    }

    #[repr(C)]
    struct ServiceFailureActionsA {
        dw_reset_period: Dword,
        lp_reboot_msg: *mut u8,
        lp_command: *mut u8,
        c_actions: Dword,
        lp_actions: *mut ScAction,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenSCManagerW(machine: *const u16, database: *const u16, access: Dword) -> Handle;
        fn CreateServiceW(
            sc_manager: Handle,
            name: *const u16,
            display: *const u16,
            access: Dword,
            service_type: Dword,
            start_type: Dword,
            error_control: Dword,
            binary_path: *const u16,
            load_order_group: *const u16,
            tag_id: *mut Dword,
            dependencies: *const u16,
            account: *const u16,
            password: *const u16,
        ) -> Handle;
        fn ChangeServiceConfigW(
            service: Handle,
            service_type: Dword,
            start_type: Dword,
            error_control: Dword,
            binary_path: *const u16,
            load_order_group: *const u16,
            tag_id: *mut Dword,
            dependencies: *const u16,
            account: *const u16,
            password: *const u16,
            display_name: *const u16,
        ) -> WinBool;
        fn ChangeServiceConfig2W(service: Handle, level: Dword, info: *const core::ffi::c_void) -> WinBool;
        fn DeleteService(service: Handle) -> WinBool;
        fn OpenServiceW(sc_manager: Handle, name: *const u16, access: Dword) -> Handle;
        fn CloseServiceHandle(handle: Handle) -> WinBool;
        fn QueryServiceStatusEx(
            service: Handle,
            level: Dword,
            buffer: *mut u8,
            size: Dword,
            needed: *mut Dword,
        ) -> WinBool;
        fn StartServiceW(service: Handle, argc: Dword, argv: *const *const u16) -> WinBool;
        fn ControlService(service: Handle, control: Dword, status: *mut ServiceStatus) -> WinBool;
        fn StartServiceCtrlDispatcherW(table: *const ServiceTableEntryW) -> WinBool;
        fn RegisterServiceCtrlHandlerExW(
            name: *const u16,
            handler: extern "system" fn(Dword, Dword, *mut core::ffi::c_void, *mut core::ffi::c_void) -> Dword,
            context: *mut core::ffi::c_void,
        ) -> Handle;
        fn SetServiceStatus(handle: Handle, status: *mut ServiceStatus) -> WinBool;
    }

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }

    static STATUS_HANDLE: AtomicIsize = AtomicIsize::new(0);
    static CURRENT_STATE: AtomicU32 = AtomicU32::new(SERVICE_STOPPED);
    static RUNNER: OnceLock<fn()> = OnceLock::new();
    static SERVICE_NAME: OnceLock<String> = OnceLock::new();

    fn report(state: Dword, wait_hint: Dword) {
        let handle = STATUS_HANDLE.load(Ordering::SeqCst) as Handle;
        CURRENT_STATE.store(state, Ordering::SeqCst);
        if handle.is_null() {
            return;
        }
        let controls = if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
        } else {
            0
        };
        let mut status = ServiceStatus {
            dw_service_type: SERVICE_WIN32_OWN_PROCESS,
            dw_current_state: state,
            dw_controls_accepted: controls,
            dw_win32_exit_code: 0,
            dw_service_specific_exit_code: 0,
            dw_check_point: 0,
            dw_wait_hint: wait_hint,
        };
        unsafe {
            SetServiceStatus(handle, &mut status);
        }
    }

    extern "system" fn ctrl_handler(
        control: Dword,
        _event_type: Dword,
        _event_data: *mut core::ffi::c_void,
        _context: *mut core::ffi::c_void,
    ) -> Dword {
        match control {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                report(SERVICE_STOP_PENDING, 5000);
                super::request_shutdown();
            }
            SERVICE_CONTROL_INTERROGATE => {
                let state = CURRENT_STATE.load(Ordering::SeqCst);
                report(state, 0);
            }
            _ => {}
        }
        0
    }

    extern "system" fn service_main(_argc: Dword, _argv: *mut *mut u16) {
        let name = wide(SERVICE_NAME.get().map(|s| s.as_str()).unwrap_or(DEFAULT_SERVICE_NAME));
        let handle = unsafe {
            RegisterServiceCtrlHandlerExW(name.as_ptr(), ctrl_handler, std::ptr::null_mut())
        };
        STATUS_HANDLE.store(handle as isize, Ordering::SeqCst);
        report(SERVICE_START_PENDING, 5000);
        report(SERVICE_RUNNING, 0);
        if let Some(run) = RUNNER.get() {
            run();
        }
        report(SERVICE_STOPPED, 0);
    }

    /// Hand over to the SCM. Blocks until the service stops.
    pub fn dispatch(name: &str, runner: fn()) -> Result<(), String> {
        let _ = SERVICE_NAME.set(name.to_string());
        let _ = RUNNER.set(runner);
        let name_w = wide(name);
        let table = [
            ServiceTableEntryW {
                lp_service_name: name_w.as_ptr(),
                lp_service_proc: Some(service_main),
            },
            ServiceTableEntryW {
                lp_service_name: std::ptr::null(),
                lp_service_proc: None,
            },
        ];
        let ok = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
        if ok == 0 {
            return Err(format!(
                "StartServiceCtrlDispatcherW failed: {} (install it with --install-service, or start it with: sc.exe start {})",
                last_error(),
                name
            ));
        }
        Ok(())
    }

    /// Strip the verbatim prefix that canonicalize() adds, so the recorded paths stay readable.
    fn pretty(path: &Path) -> String {
        let s = path.display().to_string();
        s.strip_prefix("\\\\?\\").map(|x| x.to_string()).unwrap_or(s)
    }

    pub fn install(
        config: &Path,
        log: Option<&Path>,
        name: &str,
        display_name: Option<&str>,
        account: Option<&str>,
        password: Option<&str>,
    ) -> Result<String, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot resolve the executable path: {}", e))?;
        let mut bin = format!("\"{}\" --service --config \"{}\"", pretty(&exe), pretty(config));
        if let Some(log) = log {
            bin.push_str(&format!(" --log-file \"{}\"", pretty(log)));
        }
        let display = match display_name {
            Some(d) if !d.trim().is_empty() => d.trim().to_string(),
            _ => {
                if name == DEFAULT_SERVICE_NAME {
                    PRODUCT_NAME.to_string()
                } else {
                    format!("{} ({})", PRODUCT_NAME, name)
                }
            }
        };
        let desc = description(config);
        let bin_w = wide(&bin);
        let name_w = wide(name);
        let display_w = wide(&display);
        let account_w = account.map(wide);
        let password_w = password.map(wide);
        unsafe {
            let scm = OpenSCManagerW(
                std::ptr::null(),
                std::ptr::null(),
                SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE,
            );
            if scm.is_null() {
                return Err(format!(
                    "OpenSCManagerW failed: {} (run this from an elevated (Administrator) prompt)",
                    last_error()
                ));
            }
            let existing = OpenServiceW(
                scm,
                name_w.as_ptr(),
                SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG | SERVICE_START,
            );
            // NOTE: lpdwTagId must be NULL for a Win32 own-process service, otherwise
            // CreateServiceW fails with ERROR_INVALID_PARAMETER (87). Verified on Windows 11.
            let (service, action) = if !existing.is_null() {
                let ok = ChangeServiceConfigW(
                    existing,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    bin_w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    account_w.as_ref().map(|a| a.as_ptr()).unwrap_or(std::ptr::null()),
                    password_w.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null()),
                    display_w.as_ptr(),
                );
                if ok == 0 {
                    let err = last_error();
                    CloseServiceHandle(existing);
                    CloseServiceHandle(scm);
                    return Err(format!("ChangeServiceConfigW failed: {}", err));
                }
                (existing, "updated")
            } else {
                let created = CreateServiceW(
                    scm,
                    name_w.as_ptr(),
                    display_w.as_ptr(),
                    SERVICE_QUERY_STATUS | SERVICE_CHANGE_CONFIG | SERVICE_START | SERVICE_STOP | DELETE,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    bin_w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    account_w.as_ref().map(|a| a.as_ptr()).unwrap_or(std::ptr::null()),
                    password_w.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null()),
                );
                if created.is_null() {
                    let err = last_error();
                    CloseServiceHandle(scm);
                    return Err(format!(
                        "CreateServiceW failed: {} (run this from an elevated (Administrator) prompt)",
                        err
                    ));
                }
                (created, "installed")
            };

            // description
            let mut desc_w = wide(&desc);
            let info = ServiceDescriptionW {
                lp_description: desc_w.as_mut_ptr(),
            };
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_DESCRIPTION,
                &info as *const _ as *const core::ffi::c_void,
            );
            // restart on crash: 5s, 10s, 30s
            let mut actions = [
                ScAction { kind: SC_ACTION_RESTART, delay_ms: 5000 },
                ScAction { kind: SC_ACTION_RESTART, delay_ms: 10000 },
                ScAction { kind: SC_ACTION_RESTART, delay_ms: 30000 },
            ];
            let mut failures = ServiceFailureActionsA {
                dw_reset_period: 86400,
                lp_reboot_msg: std::ptr::null_mut(),
                lp_command: std::ptr::null_mut(),
                c_actions: actions.len() as Dword,
                lp_actions: actions.as_mut_ptr(),
            };
            ChangeServiceConfig2W(
                service,
                SERVICE_CONFIG_FAILURE_ACTIONS,
                &mut failures as *mut _ as *const core::ffi::c_void,
            );
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            Ok(format!(
                "{}: service {} ({}) is {} and set to start automatically.\n  binary : {}\n  account: {}\n  log    : {}\nStart it with: sc.exe start {}   |   check with: sc.exe query {}",
                PRODUCT_NAME,
                name,
                display,
                action,
                bin,
                account.unwrap_or("LocalSystem (default)"),
                log.map(|l| pretty(l)).unwrap_or_else(|| "(from config file)".to_string()),
                name,
                name
            ))
        }
    }

    pub fn uninstall(name: &str) -> Result<String, String> {
        let name_w = wide(name);
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err(format!(
                    "OpenSCManagerW failed: {} (run this from an elevated (Administrator) prompt)",
                    last_error()
                ));
            }
            let service = OpenServiceW(
                scm,
                name_w.as_ptr(),
                SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE,
            );
            if service.is_null() {
                let err = last_error();
                CloseServiceHandle(scm);
                return Err(format!("service {} is not installed (OpenServiceW: {})", name, err));
            }
            let mut status = ServiceStatus {
                dw_service_type: 0,
                dw_current_state: 0,
                dw_controls_accepted: 0,
                dw_win32_exit_code: 0,
                dw_service_specific_exit_code: 0,
                dw_check_point: 0,
                dw_wait_hint: 0,
            };
            if ControlService(service, SERVICE_CONTROL_STOP, &mut status) != 0 {
                for _ in 0..30 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let mut buf = [0u8; 64];
                    let mut needed: Dword = 0;
                    if QueryServiceStatusEx(service, 0, buf.as_mut_ptr(), buf.len() as Dword, &mut needed) == 0 {
                        break;
                    }
                    let s = &*(buf.as_ptr() as *const ServiceStatus);
                    if s.dw_current_state == SERVICE_STOPPED {
                        break;
                    }
                }
            }
            let ok = DeleteService(service);
            let err = last_error();
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            if ok == 0 {
                return Err(format!("DeleteService failed: {}", err));
            }
            Ok(format!("{}: service {} removed", PRODUCT_NAME, name))
        }
    }

    pub fn status(name: &str) -> Result<String, String> {
        let name_w = wide(name);
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err(format!("OpenSCManagerW failed: {}", last_error()));
            }
            let service = OpenServiceW(scm, name_w.as_ptr(), SERVICE_QUERY_STATUS);
            if service.is_null() {
                let err = last_error();
                CloseServiceHandle(scm);
                return Err(format!("service {} is not installed (OpenServiceW: {})", name, err));
            }
            let mut buf = [0u8; 128];
            let mut needed: Dword = 0;
            let ok = QueryServiceStatusEx(service, 0, buf.as_mut_ptr(), buf.len() as Dword, &mut needed);
            let err = last_error();
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            if ok == 0 {
                return Err(format!("QueryServiceStatusEx failed: {}", err));
            }
            let s = &*(buf.as_ptr() as *const ServiceStatusProcess);
            let state = match s.dw_current_state {
                SERVICE_STOPPED => "STOPPED",
                SERVICE_START_PENDING => "START_PENDING",
                SERVICE_STOP_PENDING => "STOP_PENDING",
                SERVICE_RUNNING => "RUNNING",
                _ => "OTHER",
            };
            Ok(format!(
                "{}: service {} is {} (pid {})",
                PRODUCT_NAME, name, state, s.dw_process_id
            ))
        }
    }

    pub fn control(name: &str, action: &str) -> Result<String, String> {
        let name_w = wide(name);
        let access = match action {
            "stop" | "restart" => SERVICE_STOP | SERVICE_QUERY_STATUS,
            "start" => SERVICE_START | SERVICE_QUERY_STATUS,
            _ => SERVICE_QUERY_STATUS,
        };
        unsafe {
            let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err(format!("OpenSCManagerW failed: {}", last_error()));
            }
            let service = OpenServiceW(scm, name_w.as_ptr(), access);
            if service.is_null() {
                let err = last_error();
                CloseServiceHandle(scm);
                return Err(format!("service {} is not installed (OpenServiceW: {})", name, err));
            }
            let mut status = ServiceStatus {
                dw_service_type: 0,
                dw_current_state: 0,
                dw_controls_accepted: 0,
                dw_win32_exit_code: 0,
                dw_service_specific_exit_code: 0,
                dw_check_point: 0,
                dw_wait_hint: 0,
            };
            let result = match action {
                "start" => {
                    if StartServiceW(service, 0, std::ptr::null()) == 0 {
                        Err(format!("StartServiceW: {}", last_error()))
                    } else {
                        Ok("start requested")
                    }
                }
                "stop" => {
                    if ControlService(service, SERVICE_CONTROL_STOP, &mut status) == 0 {
                        Err(format!("ControlService(STOP): {}", last_error()))
                    } else {
                        Ok("stop requested")
                    }
                }
                "restart" => {
                    if ControlService(service, SERVICE_CONTROL_STOP, &mut status) == 0 {
                        Err(format!("ControlService(STOP): {}", last_error()))
                    } else {
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        if StartServiceW(service, 0, std::ptr::null()) == 0 {
                            Err(format!("StartServiceW: {}", last_error()))
                        } else {
                            Ok("restart requested")
                        }
                    }
                }
                other => Err(format!("unknown action {}", other)),
            };
            CloseServiceHandle(service);
            CloseServiceHandle(scm);
            result.map(|m| format!("{}: service {} -> {}", PRODUCT_NAME, name, m))
        }
    }

    pub fn default_binary_hint() -> PathBuf {
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ar-ocg-router.exe"))
    }
}

#[cfg(windows)]
pub use platform::{control, dispatch, install, status, uninstall};

// --------------------------------------------------------------------------- other OS

#[cfg(not(windows))]
pub fn dispatch(_name: &str, _runner: fn()) -> Result<(), String> {
    Err("service mode is Windows-only; on Linux use the shipped systemd unit (ar-ocg-router.service)".to_string())
}

#[cfg(not(windows))]
pub fn install(
    _config: &Path,
    _log: Option<&Path>,
    _name: &str,
    _display: Option<&str>,
    _account: Option<&str>,
    _password: Option<&str>,
) -> Result<String, String> {
    Err("use dist/linux-x64/install.sh (systemd) on Linux".to_string())
}

#[cfg(not(windows))]
pub fn uninstall(_name: &str) -> Result<String, String> {
    Err("use dist/linux-x64/install.sh uninstall on Linux".to_string())
}

#[cfg(not(windows))]
pub fn status(_name: &str) -> Result<String, String> {
    Err("use systemctl status ar-ocg-router on Linux".to_string())
}

#[cfg(not(windows))]
pub fn control(_name: &str, _action: &str) -> Result<String, String> {
    Err("use systemctl start/stop/restart ar-ocg-router on Linux".to_string())
}
