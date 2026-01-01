use std::ffi::CString;
use std::path::PathBuf;
use std::time::Duration;
use std::{mem, ptr, thread};
use tracing::{debug, error, info};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CREATE_TOOLHELP_SNAPSHOT_FLAGS, CreateToolhelp32Snapshot, MODULEENTRY32W, PROCESSENTRY32W,
    Module32FirstW, Module32NextW, Process32FirstW, Process32NextW,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
use windows::Win32::System::Threading::{
    GetCurrentThread, GetExitCodeThread, GetProcessId, OpenProcess, QueryFullProcessImageNameW,
    SetThreadPriority, WaitForInputIdle, WaitForSingleObject, PROCESS_NAME_FORMAT,
    THREAD_PRIORITY_HIGHEST,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
use windows::core::{BOOL, s};

// Define function pointer types for the dynamically resolved NT functions
type ZwAllocateVirtualMemoryFn = unsafe extern "system" fn(
    HANDLE,
    *mut *mut core::ffi::c_void,
    usize,
    *mut usize,
    u32,
    u32,
) -> i32;

type ZwWriteVirtualMemoryFn = unsafe extern "system" fn(
    HANDLE,
    *mut core::ffi::c_void,
    *const core::ffi::c_void,
    usize,
    *mut usize,
) -> i32;

type ZwCreateThreadExFn = unsafe extern "system" fn(
    *mut HANDLE,
    u32,
    *const core::ffi::c_void,
    HANDLE,
    *mut core::ffi::c_void,
    *mut core::ffi::c_void,
    i32,
    usize,
    usize,
    usize,
    *const core::ffi::c_void,
) -> i32;

pub unsafe fn inject_dll_to_handle(ph: HANDLE, dll_path: &str) -> bool {
    unsafe {
        // 1. Get Handle to Kernel32
        let h_kernel32 = match GetModuleHandleA(s!("Kernel32")) {
            Ok(handle) => handle,
            Err(_) => {
                // Assuming GetLastError is available (check your imports if this fails)
                // Note: In 0.62, GetLastError is in Foundation.
                error!("Failed to get Kernel32 handle");
                return false;
            }
        };

        // 2. Get Handle to ntdll
        let h_ntdll = match GetModuleHandleA(s!("ntdll.dll")) {
            Ok(handle) => handle,
            Err(_) => {
                error!("Failed to get ntdll handle");
                return false;
            }
        };

        // 3. Get LoadLibraryA address
        let lb = GetProcAddress(h_kernel32, s!("LoadLibraryA"));
        if lb.is_none() {
            error!("Failed to get LoadLibraryA address");
            return false;
        }

        // 4. Resolve NT function addresses
        let zw_alloc_virtual_mem_addr = GetProcAddress(h_ntdll, s!("ZwAllocateVirtualMemory"));
        let zw_write_virtual_mem_addr = GetProcAddress(h_ntdll, s!("ZwWriteVirtualMemory"));
        let zw_create_thread_ex_addr = GetProcAddress(h_ntdll, s!("ZwCreateThreadEx"));

        if zw_write_virtual_mem_addr.is_none()
            || zw_create_thread_ex_addr.is_none()
            || zw_alloc_virtual_mem_addr.is_none()
        {
            error!("Failed to get Windows function address");
            return false;
        }

        // Cast function pointers to the types defined above
        let zw_allocate_virtual_memory: ZwAllocateVirtualMemoryFn =
            mem::transmute(zw_alloc_virtual_mem_addr.unwrap());
        let zw_write_virtual_memory: ZwWriteVirtualMemoryFn =
            mem::transmute(zw_write_virtual_mem_addr.unwrap());
        let zw_create_thread_ex: ZwCreateThreadExFn =
            mem::transmute(zw_create_thread_ex_addr.unwrap());

        // 5. Prepare DLL Path
        let dll_path_c = match CString::new(dll_path) {
            Ok(s) => s,
            Err(_) => {
                error!("Failed to convert DLL path to CString");
                return false;
            }
        };
        let dll_path_ptr = dll_path_c.as_ptr() as *const core::ffi::c_void;
        let dll_length = dll_path_c.as_bytes_with_nul().len();

        // 6. Allocate Memory
        let mut base_address: *mut core::ffi::c_void = ptr::null_mut();
        let mut region_size = dll_length;

        let status = zw_allocate_virtual_memory(
            ph,
            &mut base_address,
            0,
            &mut region_size,
            MEM_RESERVE.0 | MEM_COMMIT.0,
            PAGE_READWRITE.0,
        );

        if status != 0 {
            error!(
                "Failed to allocate memory in the target process: 0x{:X}",
                status
            );
            return false;
        }

        let rb = base_address;

        // 7. Write DLL Path
        let write_status =
            zw_write_virtual_memory(ph, rb, dll_path_ptr, dll_length, ptr::null_mut());

        if write_status != 0 {
            error!(
                "Failed to write memory in the target process: 0x{:X}",
                write_status
            );
            return false;
        }

        // 8. Create Remote Thread
        let mut h_thread: HANDLE = HANDLE::default();

        let create_status = zw_create_thread_ex(
            &mut h_thread,
            windows::Win32::System::Threading::THREAD_ALL_ACCESS.0,
            ptr::null(),
            ph,
            lb.unwrap() as *mut core::ffi::c_void,
            rb,
            0,
            0,
            0,
            0,
            ptr::null(),
        );

        if create_status == 0 && !h_thread.is_invalid() {
            info!("Remote thread created. Waiting for completion...");
            WaitForSingleObject(h_thread, 5000);

            let mut exit_code: u32 = 0;
            let _ = GetExitCodeThread(h_thread, &mut exit_code);

            if exit_code == 0 {
                error!("LoadLibraryA failed in target process (Exit code 0). Check if DLL path is correct and architecture matches.");
                false
            } else {
                info!("LoadLibraryA succeeded. DLL base address: 0x{:X}", exit_code);
                true
            }
        } else {
            error!(
                "Failed to create thread in the target process: 0x{:X}",
                create_status
            );
            false
        }
    }
}

pub fn wait_for_process(target_process_name: &str, _dll_path: &str) -> HANDLE {
    // 1. Convert the borrowed string slice (&str) to a Wide String
    // This is efficient: it only allocates the memory for the UTF-16 vector.
    let target_process_name = target_process_name.trim_end_matches('\0');
    let mut target_name_wide: Vec<u16> = target_process_name.encode_utf16().collect();
    target_name_wide.push(0); // Manually add null terminator

    info!("Waiting for '{}'...", target_process_name);

    unsafe {
        // 2. Boost timer resolution to 1ms
        let _ = timeBeginPeriod(1);
        // 3. Set high priority
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }

    loop {
        unsafe {
            let snapshot = match CreateToolhelp32Snapshot(CREATE_TOOLHELP_SNAPSHOT_FLAGS(0x00000002), 0) {
                Ok(h) => h,
                Err(_) => {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
            };

            let mut entry = PROCESSENTRY32W::default();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    if entry.szExeFile.starts_with(&target_name_wide) {
                        info!("Process found! PID: {}", entry.th32ProcessID);

                        let ph = match OpenProcess(
                            windows::Win32::System::Threading::PROCESS_ALL_ACCESS,
                            false,
                            entry.th32ProcessID,
                        ) {
                            Ok(handle) => handle,
                            Err(e) => {
                                error!("OpenProcess failed (Admin?): {}", e);
                                thread::sleep(Duration::from_millis(1));
                                break;
                            }
                        };

                        return ph;
                    }

                    if Process32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
        }

        // 5. Sleep 1ms (Accuracy guaranteed by timeBeginPeriod)
        thread::sleep(Duration::from_millis(1));
    }
}

// May be useful for future features
#[allow(dead_code)]
pub fn get_process_directory(ph: HANDLE) -> Option<PathBuf> {
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;

    unsafe {
        if QueryFullProcessImageNameW(ph, PROCESS_NAME_FORMAT(0), windows::core::PWSTR(buffer.as_mut_ptr()), &mut size).is_ok() {
            let path_str = String::from_utf16_lossy(&buffer[..size as usize]);
            let path = PathBuf::from(path_str);
            return path.parent().map(|p| p.to_path_buf());
        }
    }
    None
}

#[allow(dead_code)]
pub fn wait_for_module(ph: HANDLE, module_name: &str) -> bool {
    let pid = unsafe { GetProcessId(ph) };
    let module_name_lower = module_name.to_lowercase();
    debug!("Waiting for module '{}' in PID {}...", module_name, pid);

    loop {
        unsafe {
            // TH32CS_SNAPMODULE (0x8) | TH32CS_SNAPMODULE32 (0x10)
            let snapshot = match CreateToolhelp32Snapshot(CREATE_TOOLHELP_SNAPSHOT_FLAGS(0x00000008 | 0x00000010), pid) {
                Ok(h) => h,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };

            let mut entry = MODULEENTRY32W::default();
            entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;

            if Module32FirstW(snapshot, &mut entry).is_ok() {
                loop {
                    let current_module = String::from_utf16_lossy(&entry.szModule);
                    let current_module = current_module.trim_matches('\0').to_lowercase();

                    if current_module == module_name_lower {
                        info!("Module '{}' found and initialized!", module_name);
                        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
                        return true;
                    }

                    if Module32NextW(snapshot, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = windows::Win32::Foundation::CloseHandle(snapshot);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

struct EnumData {
    target_pid: u32,
    found: bool,
}

unsafe extern "system" fn enum_windows_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let data = unsafe { &mut *(lparam.0 as *mut EnumData) };
    let mut window_pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };

    if window_pid == data.target_pid && unsafe { IsWindowVisible(hwnd).as_bool() } {
        data.found = true;
        return BOOL::from(false); // Stop enumerating
    }

    BOOL::from(true) // Continue enumerating
}

pub fn wait_for_window(ph: HANDLE) {
    let target_pid = unsafe { GetProcessId(ph) };
    debug!("Waiting for a visible window for PID {}...", target_pid);

    let mut data = EnumData {
        target_pid,
        found: false,
    };

    loop {
        unsafe {
            let _ = EnumWindows(Some(enum_windows_callback), LPARAM(&mut data as *mut EnumData as isize));
        }

        if data.found {
            info!("Visible window found!");
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

pub fn wait_for_input_idle(ph: HANDLE, timeout_ms: u32) -> u32 {
    unsafe { WaitForInputIdle(ph, timeout_ms) }
}
