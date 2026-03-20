use crate::rpc_client::rpc_call;

pub(crate) async fn rpc_call_in_background(
    method: &'static str,
    addr: Option<String>,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let method_name = method.to_string();
    let method_for_task = method_name.clone();
    tauri::async_runtime::spawn_blocking(move || rpc_call(&method_for_task, addr, params))
        .await
        .map_err(|err| format!("{method_name} task failed: {err}"))?
}

pub(crate) fn open_in_browser_blocking(url: &str) -> Result<(), String> {
    if cfg!(target_os = "windows") {
        let status = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("rundll32 failed with status: {status}"))
        }
    } else {
        webbrowser::open(url).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn spawn_background_command(
    mut command: std::process::Command,
    launch_failure_message: &str,
) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|err| format!("{launch_failure_message}：{err}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(crate) fn open_in_file_manager_blocking(path: &str) -> Result<(), String> {
    let normalized = path.trim();
    if normalized.is_empty() {
        return Err("缺少要打开的目录".to_string());
    }

    let target = std::path::PathBuf::from(normalized);
    if !target.exists() {
        return Err(format!("目录不存在：{}", target.display()));
    }

    let dir = if target.is_dir() {
        target
    } else {
        target
            .parent()
            .map(|value| value.to_path_buf())
            .ok_or_else(|| format!("无法解析目录：{}", normalized))?
    };

    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(dir.as_os_str());
        spawn_background_command(command, "打开资源管理器失败")
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        command.arg(dir.as_os_str());
        spawn_background_command(command, "打开 Finder 失败")
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(dir.as_os_str());
        spawn_background_command(command, "打开目录失败")
    }
}
