use super::*;

#[test]
fn partial_inventory_keeps_fallback_cpu_and_unknown_gpu() {
    let mut info = json!({"cpu":8,"gpu":null,"ram_mb":null});
    merge(&mut info, &json!({"cpu":0,"ram_mb":8192}));
    assert_eq!(info["cpu"], 8);
    assert!(info["gpu"].is_null());
    merge(&mut info, &json!({"gpu":0}));
    assert_eq!(info["gpu"], 0);
}

#[test]
fn pci_gpu_detection_distinguishes_absence_from_unavailable() {
    let root = std::env::temp_dir().join(format!("ax-gpu-test-{}", uuid::Uuid::new_v4()));
    assert!(linux_gpus(&root).0.is_none());
    std::fs::create_dir_all(root.join("gpu")).unwrap();
    std::fs::write(root.join("gpu/class"), "0x030200\n").unwrap();
    std::fs::write(root.join("gpu/vendor"), "0x10de\n").unwrap();
    std::fs::write(root.join("gpu/device"), "0x1234\n").unwrap();
    assert_eq!(linux_gpus(&root).0, Some(1));
    std::fs::write(root.join("gpu/class"), "0x020000\n").unwrap();
    assert_eq!(linux_gpus(&root).0, Some(0));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_probe_returns_bounded_valid_inventory() {
    let info = detect().await;
    assert!(info["cpu"].as_u64().unwrap() > 0);
    assert!(info["errors"].is_array());
    assert!(info["gpu"].is_null() || info["gpu"].is_u64());
    eprintln!("Detected Host: {info}");
}
