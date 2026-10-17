//! What `wfd-flash` writes to a clock over USB (src/bin/wfd-flash.rs), as plain logic.
//!
//! The release's merged image (`wfd-clock-merged.bin`: bootloader, partition table, otadata,
//! app) is meant for flash address 0 and fills the gaps between its parts with 0xFF, so writing
//! it whole also erases the settings partition (`nvs`: WiFi and every setting). That's right for
//! a clock on the stock firmware (its settings are in the backup), wrong for one already on ours.
//! [`plan`] writes the whole image to a stock clock and skips `nvs` on one that runs ours.

/// Where the ESP32 bootloader finds the partition table.
pub const PARTITION_TABLE: usize = 0x8000;
/// The clock's flash size (4 MB): a backup is all of it.
pub const FLASH_SIZE: u32 = 0x40_0000;

/// One partition table entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Partition {
    pub name: String,
    pub kind: u8,    // 0 app, 1 data
    pub subtype: u8, // data: 0 ota, 2 nvs, ...
    pub offset: u32,
    pub size: u32,
}

/// The partition table inside a merged image (32-byte entries starting 0xAA 0x50).
pub fn partitions(image: &[u8]) -> Vec<Partition> {
    let mut out = Vec::new();
    let mut at = PARTITION_TABLE;
    while let Some(e) = image.get(at..at + 32) {
        if e[0] != 0xAA || e[1] != 0x50 {
            break;
        }
        let u32_at = |i: usize| u32::from_le_bytes([e[i], e[i + 1], e[i + 2], e[i + 3]]);
        let name_end = e[12..28].iter().position(|&b| b == 0).unwrap_or(16);
        out.push(Partition {
            name: String::from_utf8_lossy(&e[12..12 + name_end]).into_owned(),
            kind: e[2],
            subtype: e[3],
            offset: u32_at(4),
            size: u32_at(8),
        });
        at += 32;
    }
    out
}

/// The settings partition (data/nvs) of a merged image.
pub fn nvs(image: &[u8]) -> Option<Partition> {
    partitions(image).into_iter().find(|p| p.kind == 1 && p.subtype == 2)
}

/// A piece of the image to write: (flash address, bytes).
pub type Segment<'a> = (u32, &'a [u8]);

/// What to write for a clock that runs our firmware (`keep_settings`) or anything else.
/// Errors if the image has no settings partition to skip, or it lies outside the image.
pub fn plan(image: &[u8], keep_settings: bool) -> Result<Vec<Segment<'_>>, String> {
    if image.len() as u32 > FLASH_SIZE {
        return Err("the image is bigger than the clock's flash".into());
    }
    if !keep_settings {
        return Ok(vec![(0, image)]);
    }
    let nvs = nvs(image).ok_or("the image has no settings (nvs) partition")?;
    let (start, end) = (nvs.offset as usize, (nvs.offset + nvs.size) as usize);
    if end > image.len() {
        return Err("the settings partition isn't inside the image".into());
    }
    Ok(vec![(0, &image[..start]), (end as u32, &image[end..])])
}

/// Did the clock answer like our firmware? (`CFG` replies `CFG {json}`.) Returns its build.
pub fn our_firmware(reply: &str) -> Option<String> {
    let line = reply.lines().find_map(|l| l.trim().strip_prefix("CFG "))?;
    let cfg: serde_json::Value = serde_json::from_str(line).ok()?;
    Some(cfg.get("_fw").and_then(|v| v.as_str()).unwrap_or("an older build").to_string())
}

/// A backup's file name, from a local timestamp ("20261017-141500").
pub fn backup_name(stamp: &str) -> String {
    format!("clock-backup-{stamp}.bin")
}
