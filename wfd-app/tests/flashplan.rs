//! What wfd-flash writes (src/flashplan.rs), against the real shipped image.
use wfd_app::flashplan::{FLASH_SIZE, backup_name, nvs, our_firmware, partitions, plan};

static MERGED: &[u8] = include_bytes!("../../wfd-firmware/releases/wfd-clock-merged.bin");

#[test]
fn the_image_has_the_clocks_partition_table() {
    let names: Vec<String> = partitions(MERGED).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["nvs", "otadata", "app0", "app1", "spiffs"]);
    let nvs = nvs(MERGED).unwrap();
    assert_eq!((nvs.offset, nvs.size), (0x9000, 0x5000), "must match partitions_wfd.csv");
}

#[test]
fn the_image_would_erase_the_settings_if_written_whole() {
    // why plan() exists: the merged image fills the nvs gap with 0xFF
    assert!(MERGED[0x9000..0xE000].iter().all(|&b| b == 0xFF));
}

#[test]
fn a_stock_clock_gets_the_whole_image() {
    let p = plan(MERGED, false).unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].0, 0);
    assert_eq!(p[0].1.len(), MERGED.len());
}

#[test]
fn our_clock_keeps_its_settings() {
    let p = plan(MERGED, true).unwrap();
    assert_eq!(p.len(), 2);
    let (a0, d0) = p[0];
    let (a1, d1) = p[1];
    assert_eq!((a0, d0.len()), (0, 0x9000), "bootloader + partition table, up to nvs");
    assert_eq!(a1, 0xE000, "otadata onwards (resets the boot slot to app0, where the new app goes)");
    assert_eq!(d1.len(), MERGED.len() - 0xE000);
    assert_eq!(d1, &MERGED[0xE000..]);
    // nothing written inside nvs
    for (addr, data) in p {
        let end = addr as usize + data.len();
        assert!(end <= 0x9000 || addr as usize >= 0xE000, "{addr:#x}..{end:#x} touches nvs");
    }
}

#[test]
fn bad_images_are_refused() {
    assert!(plan(&[0xFFu8; 0x100], true).is_err(), "no partition table");
    let too_big = vec![0u8; FLASH_SIZE as usize + 1];
    assert!(plan(&too_big, false).is_err());
}

#[test]
fn recognises_our_firmware_by_its_cfg_reply() {
    let reply = "boot noise\r\nCFG {\"brightness\":7,\"_fw\":\"wfd-clock Oct 14 2026 14:36:22\",\"_protocol\":4}\r\n";
    assert_eq!(our_firmware(reply).as_deref(), Some("wfd-clock Oct 14 2026 14:36:22"));
    assert_eq!(our_firmware("CFG {\"brightness\":7}\n").as_deref(), Some("an older build"));
    assert_eq!(our_firmware("ets Jun  8 2016 00:22:57\nrst:0x1\n"), None, "the stock firmware");
    assert_eq!(our_firmware("CFG {broken\n"), None);
}

#[test]
fn backups_are_named_by_time() {
    assert_eq!(backup_name("20261017-141500"), "clock-backup-20261017-141500.bin");
}
