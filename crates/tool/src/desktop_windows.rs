//! Native UIA objects stay inside a disposable helper process and COM apartment.
use enigo::Keyboard;
use serde_json::{Value, json};
use std::collections::VecDeque;
use uiautomation::{
    UIAutomation, UIElement, UITreeWalker,
    inputs::Mouse,
    patterns::{
        UIExpandCollapsePattern, UIInvokePattern, UISelectionItemPattern, UITogglePattern,
        UIValuePattern,
    },
    types::{Handle, Point, TreeScope},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const PRIVACY_LIMIT: usize = 20_000;

fn application(window: &UIElement) -> Result<crate::HostTarget> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let pid = Pid::from_u32(window.get_process_id()?);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let path = system
        .process(pid)
        .and_then(sysinfo::Process::exe)
        .ok_or("Cannot verify target application's executable")?;
    let path = std::fs::canonicalize(path)?;
    Ok(crate::HostTarget {
        surface: crate::HostSurface::Computer,
        id: crate::app_id(&path)?,
        label: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    })
}

fn id(element: &UIElement) -> Result<String> {
    Ok(element
        .get_runtime_id()?
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("."))
}
fn short(text: &str) -> String {
    text.chars().take(2048).collect()
}
fn optional_element(value: uiautomation::Result<UIElement>) -> Result<Option<UIElement>> {
    // UIA returns a null interface with S_OK (or E_POINTER) for no child.
    // windows-rs represents that null success as an error with code zero.
    // Other failures must
    // propagate, especially when checking whether screenshots are safe.
    match value {
        Ok(element) => Ok(Some(element)),
        Err(error) if error.code() == 0 || error.code() == -2_147_467_261 => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn enqueue(
    walker: &UITreeWalker,
    node: &UIElement,
    queue: &mut VecDeque<UIElement>,
    visited: usize,
) -> Result<()> {
    let mut next = optional_element(walker.get_first_child(node))?;
    while let Some(child) = next {
        if queue.len() + visited >= PRIVACY_LIMIT {
            return Err("Accessibility scan exceeds the built-in limit".into());
        }
        next = optional_element(walker.get_next_sibling(&child))?;
        queue.push_back(child);
    }
    Ok(())
}
fn find(walker: &UITreeWalker, window: &UIElement, target: &str) -> Result<UIElement> {
    let mut queue = VecDeque::from([window.clone()]);
    let mut visited = 0;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if id(&node)? == target {
            return Ok(node);
        }
        enqueue(walker, &node, &mut queue, visited)?;
    }
    Err("Element no longer exists; read the window again".into())
}
fn belongs(
    automation: &UIAutomation,
    walker: &UITreeWalker,
    element: UIElement,
    window: &UIElement,
) -> Result<bool> {
    let mut node = Some(element);
    for _ in 0..128 {
        let Some(current) = node else {
            return Ok(false);
        };
        if automation.compare_elements(&current, window)? {
            return Ok(true);
        }
        node = optional_element(walker.get_parent(&current))?;
    }
    Ok(false)
}
fn require_focus(
    automation: &UIAutomation,
    walker: &UITreeWalker,
    window: &UIElement,
) -> Result<()> {
    if !belongs(
        automation,
        walker,
        automation.get_focused_element()?,
        window,
    )? {
        return Err("Target window is not foreground; use focus_window first".into());
    }
    Ok(())
}
fn screenshot(
    walker: &UITreeWalker,
    window: &UIElement,
    handle: isize,
    request: &Value,
    result: &mut Value,
) {
    if request["include_screenshot"] != true {
        result["screenshot_skipped"] = json!("disabled");
        return;
    }
    let privacy = || -> Result<bool> {
        let mut queue = VecDeque::from([window.clone()]);
        let mut visited = 0;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            if node.is_password()? {
                return Ok(false);
            }
            enqueue(walker, &node, &mut queue, visited)?;
        }
        Ok(true)
    };
    match privacy() {
        Ok(false) => {
            result["screenshot_skipped"] = json!("password_field");
            return;
        }
        Err(_) => {
            result["screenshot_skipped"] = json!("privacy_scan_failed");
            return;
        }
        Ok(true) => {}
    }
    let capture = || -> Result<String> {
        let bounds = window.get_bounding_rectangle()?;
        let w = i64::from(bounds.get_right()) - i64::from(bounds.get_left());
        let h = i64::from(bounds.get_bottom()) - i64::from(bounds.get_top());
        if w <= 0
            || h <= 0
            || w > 8192
            || h > 8192
            || w * h > 32_000_000
            || window.is_offscreen()?
        {
            return Err("Window is minimized or exceeds capture bounds".into());
        }
        // PrintWindow captures the target itself; never fall back to desktop
        // BitBlt, which could capture unrelated windows covering the target.
        let capture = win_screenshot::capture::capture_window(handle)?;
        let mut pixels = capture.pixels;
        // GDI's reserved alpha byte is undefined; saved windows must be opaque.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let rgba = image::RgbaImage::from_raw(capture.width, capture.height, pixels)
            .ok_or("Invalid capture pixels")?;
        let limit = u32::try_from(request["screenshot_width"].as_u64().unwrap_or(1280))?;
        let original = image::DynamicImage::ImageRgba8(rgba);
        let image = if original.width().max(original.height()) > limit {
            original.thumbnail(limit, limit)
        } else {
            original
        };
        let path = std::env::temp_dir().join(format!("ax-desktop-{}.png", uuid::Uuid::new_v4()));
        image.save(&path)?;
        Ok(path.to_string_lossy().into_owned())
    };
    match capture() {
        Ok(path) => result["screenshot_path"] = json!(path),
        Err(error) => {
            result["screenshot_skipped"] = json!("capture_unavailable");
            result["screenshot_message"] = json!(error.to_string());
        }
    }
}
fn tree(walker: &UITreeWalker, window: &UIElement, limit: usize) -> Result<Value> {
    let mut nodes = Vec::new();
    let mut queue = VecDeque::from([(window.clone(), None::<String>, 0)]);
    let mut truncated = false;
    while let Some((element, parent, depth)) = queue.pop_front() {
        let id = id(&element)?;
        // An unreadable privacy property never permits reading the value.
        let password = element.is_password().unwrap_or(true);
        let mut node = json!({"id":id,"parent_id":parent,"role":format!("{:?}",element.get_control_type()?),"name":if password { "[password]".to_owned() } else { short(&element.get_name()?) },"password":password});
        node["focused"] = json!(element.has_keyboard_focus().unwrap_or(false));
        node["enabled"] = json!(element.is_enabled().unwrap_or(false));
        if !password
            && let Ok(value) = element.get_pattern::<UIValuePattern>()
            && let Ok(value) = value.get_value()
        {
            node["value"] = json!(short(&value));
        }
        if let Ok(rect) = element.get_bounding_rectangle() {
            node["bounds"] = json!({"x":rect.get_left(),"y":rect.get_top(),"width":i64::from(rect.get_right())-i64::from(rect.get_left()),"height":i64::from(rect.get_bottom())-i64::from(rect.get_top())});
        }
        nodes.push(node);
        let mut child = optional_element(walker.get_first_child(&element))?;
        while let Some(element) = child {
            if depth >= 128 || nodes.len() + queue.len() >= limit {
                truncated = true;
                break;
            }
            child = optional_element(walker.get_next_sibling(&element))?;
            queue.push_back((element, Some(id.clone()), depth + 1));
        }
    }
    Ok(json!({"nodes":nodes,"node_limit":limit,"truncated":truncated}))
}

// The action dispatch is intentionally one table; helpers own tree/privacy work.
#[allow(clippy::too_many_lines)]
pub(super) fn run(request: &Value) -> Result<Value> {
    let automation = UIAutomation::new()?;
    let walker = automation.get_raw_view_walker()?;
    let input = &request["input"];
    let action = input["action"].as_str().ok_or("Missing action")?;
    if action == "list_windows" {
        let policy = crate::HostPermissionStore::new(crate::host_home());
        let root = automation.get_root_element()?;
        let windows = root.find_all(TreeScope::Children, &automation.create_true_condition()?)?;
        let mut entries = Vec::new();
        for window in windows {
            // Other applications can disappear while enumerating the desktop.
            // Skip stale/unreadable metadata without relaxing policy errors.
            let (Ok(handle), Ok(offscreen), Ok(process_id)) = (
                window.get_native_window_handle(),
                window.is_offscreen(),
                window.get_process_id(),
            ) else {
                continue;
            };
            let handle: isize = handle.into();
            let visible = !offscreen;
            if handle == 0 || (!visible && input["include_hidden"] != true) {
                continue;
            }
            let Ok(app) = application(&window) else {
                continue;
            };
            let access = policy.decision(&app)?;
            if access == crate::PermissionDecision::Deny {
                continue;
            }
            let title = if access == crate::PermissionDecision::Allow {
                short(&window.get_name().unwrap_or_default())
            } else {
                String::new()
            };
            entries.push(json!({"id":handle.to_string(),"title":title,"visible":visible,"process_id":process_id,"app_id":app.id,"app_name":app.label,"access":access}));
        }
        return Ok(json!({"status":"windows_listed","windows":entries}));
    }
    let handle = input["window_id"]
        .as_str()
        .ok_or("Missing window_id")?
        .parse::<isize>()?;
    let window = automation.element_from_handle(Handle::from(handle))?;
    if window.get_process_id()? == 0 {
        return Err("Window no longer exists; list windows again".into());
    }
    let app = application(&window)?;
    if request["resolve_only"] == true {
        return Ok(json!({"app_id":app.id,"app_name":app.label}));
    }
    let policy = crate::HostPermissionStore::new(crate::host_home());
    if policy.decision(&app)? == crate::PermissionDecision::Deny
        || request["authorized_app"].as_str() != Some(app.id.as_str())
    {
        return Err("Application access is not authorized; request app access first".into());
    }
    let mut result = json!({"status":"completed","window_id":handle.to_string()});
    match action {
        "read_window" => {
            result["status"] = json!("window_read");
            result["ui_tree"] = tree(
                &walker,
                &window,
                usize::try_from(request["max_nodes"].as_u64().unwrap_or(1200))?,
            )?;
            screenshot(&walker, &window, handle, request, &mut result);
        }
        "screenshot_window" => {
            screenshot(&walker, &window, handle, request, &mut result);
            result["status"] = json!("window_capture");
        }
        "focus_window" => {
            window.set_focus()?;
            require_focus(&automation, &walker, &window)?;
        }
        "click_element" => {
            let element = find(
                &walker,
                &window,
                input["element_id"].as_str().ok_or("Missing element_id")?,
            )?;
            if let Ok(pattern) = element.get_pattern::<UIInvokePattern>() {
                pattern.invoke()?;
            } else if let Ok(pattern) = element.get_pattern::<UITogglePattern>() {
                pattern.toggle()?;
            } else if let Ok(pattern) = element.get_pattern::<UISelectionItemPattern>() {
                pattern.select()?;
            } else if let Ok(pattern) = element.get_pattern::<UIExpandCollapsePattern>() {
                pattern.expand()?;
            } else {
                return Err("Element has no supported action pattern; inspect its children or use coordinates".into());
            }
        }
        "type_text" => {
            let text = input["text"].as_str().ok_or("text must be a string")?;
            let element = if let Some(id) = input["element_id"].as_str() {
                find(&walker, &window, id)?
            } else {
                let focused = automation.get_focused_element()?;
                if !belongs(&automation, &walker, focused.clone(), &window)? {
                    return Err("Focused element is outside target window".into());
                }
                focused
            };
            if let Ok(pattern) = element.get_pattern::<UIValuePattern>() {
                pattern.set_value(text)?;
            } else {
                require_focus(&automation, &walker, &window)?;
                element.set_focus()?;
                require_focus(&automation, &walker, &window)?;
                enigo::Enigo::new(&enigo::Settings::default())?.text(text)?;
            }
        }
        "press_key" => {
            require_focus(&automation, &walker, &window)?;
            let key = input["key"]
                .as_str()
                .ok_or("Missing key")?
                .to_ascii_uppercase();
            let code = match key.as_str() {
                "RETURN" | "ENTER" => 0x0D,
                "ESCAPE" | "ESC" => 0x1B,
                "TAB" => 0x09,
                "BACKSPACE" => 0x08,
                "DELETE" => 0x2E,
                "LEFT" => 0x25,
                "UP" => 0x26,
                "RIGHT" => 0x27,
                "DOWN" => 0x28,
                "HOME" => 0x24,
                "END" => 0x23,
                "PAGEUP" => 0x21,
                "PAGEDOWN" => 0x22,
                "SPACE" => 0x20,
                _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
                    u32::from(key.as_bytes()[0])
                }
                _ if key.starts_with('F') => key[1..]
                    .parse::<u32>()
                    .ok()
                    .filter(|n| (1..=12).contains(n))
                    .map(|n| 0x6F + n)
                    .ok_or("Unsupported key")?,
                _ => return Err("Unsupported key".into()),
            };
            let mut modifiers = Vec::new();
            if let Some(values) = input["modifiers"].as_array() {
                for value in values {
                    let modifier = match value.as_str() {
                        Some("ctrl") => enigo::Key::Control,
                        Some("shift") => enigo::Key::Shift,
                        Some("alt") => enigo::Key::Alt,
                        _ => return Err("Unsupported modifier".into()),
                    };
                    if !modifiers.contains(&modifier) {
                        modifiers.push(modifier);
                    }
                }
            } else if input.get("modifiers").is_some() {
                return Err("modifiers must be an array".into());
            }
            // Enigo tracks pressed keys and releases them on Drop, including
            // failure paths. Avoid parsing model text as a key-expression DSL.
            let mut keyboard = enigo::Enigo::new(&enigo::Settings::default())?;
            for modifier in &modifiers {
                keyboard.key(*modifier, enigo::Direction::Press)?;
            }
            let sent = keyboard.key(enigo::Key::Other(code), enigo::Direction::Click);
            for modifier in modifiers.iter().rev() {
                keyboard.key(*modifier, enigo::Direction::Release)?;
            }
            sent?;
        }
        "move_mouse" | "mouse_click" => {
            require_focus(&automation, &walker, &window)?;
            let x = i32::try_from(input["x"].as_i64().ok_or("x must be an integer")?)?;
            let y = i32::try_from(input["y"].as_i64().ok_or("y must be an integer")?)?;
            let rect = window.get_bounding_rectangle()?;
            if x < rect.get_left()
                || x >= rect.get_right()
                || y < rect.get_top()
                || y >= rect.get_bottom()
            {
                return Err("Coordinates are outside target window".into());
            }
            let point = Point::new(x, y);
            if !belongs(
                &automation,
                &walker,
                automation.element_from_point(point)?,
                &window,
            )? {
                return Err("Coordinates are covered by another window".into());
            }
            Mouse::set_cursor_pos(&point)?;
            if action == "mouse_click" {
                require_focus(&automation, &walker, &window)?;
                let button = match input["button"].as_str() {
                    Some("left") => uiautomation::inputs::MouseButton::LEFT,
                    Some("right") => uiautomation::inputs::MouseButton::RIGHT,
                    Some("middle") => uiautomation::inputs::MouseButton::MIDDLE,
                    _ => return Err("Use left, right or middle mouse button".into()),
                };
                Mouse::new().click_button(button)?;
            }
        }
        _ => return Err("Unsupported desktop action".into()),
    }
    Ok(result)
}
