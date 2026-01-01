mod win32;

use std::path::Path;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    warn!("Tamamo-X v{} | GitHub: https://github.com/teppyboy/tamamo-x", env!("CARGO_PKG_VERSION"));
    warn!("This is an experimental software, hence I will NOT be responsible for any damage. Use at your own risk.");

    let process_name = "UmamusumePrettyDerby.exe";
    let dll_name = "hachimi.dll";
    
    let dll_path = Path::new(dll_name);
    let absolute_dll_path = if dll_path.is_relative() {
        std::env::current_dir().unwrap().join(dll_path)
    } else {
        dll_path.to_path_buf()
    };

    let dll_path_str = absolute_dll_path.to_str().expect("Invalid DLL path");

    let target_process_handle = win32::wait_for_process(process_name, dll_path_str);
    if target_process_handle.is_invalid() {
        error!("Could not obtain handle to target process.");
        return;
    }

    // Wait for the game window to appear
    info!("Waiting for game window...");
    win32::wait_for_window(target_process_handle);
    
    // Wait until the process is in an idle state
    info!("Waiting for process to become idle...");
    win32::wait_for_input_idle(target_process_handle, 10000);

    info!("Waiting for around 1 second to ensure stability...");
    std::thread::sleep(std::time::Duration::from_millis(1000));

    let injection_result = unsafe { win32::inject_dll_to_handle(target_process_handle, dll_path_str) };
    if injection_result {
        info!("DLL Injection succeeded.");
    } else {
        error!("DLL Injection failed.");
    }
}
