//! GPU load from Windows' own performance counters (`\GPU Engine(*)\Utilization Percentage`),
//! the numbers Task Manager shows. Covers every GPU in the PC (NVIDIA, AMD, Intel) and needs no
//! helper program.
//!
//! Windows reports one value per process per engine (3D, video decode, compute, ... of each
//! adapter). An engine's load is the sum over the processes using it; the PC's GPU load is its
//! busiest engine, like Task Manager's headline figure.

use std::collections::HashMap;

/// The engine an instance belongs to: its name without the process part.
/// `pid_4012_luid_0x00000000_0x0000D3E2_phys_0_eng_3_engtype_VideoDecode` ->
/// `luid_0x00000000_0x0000D3E2_phys_0_eng_3`.
fn engine_of(instance: &str) -> Option<&str> {
    let start = instance.find("luid_")?;
    let end = instance.find("_engtype").unwrap_or(instance.len());
    (end > start).then(|| &instance[start..end])
}

/// The busiest engine's load in percent (0..100), from (instance name, utilization) samples.
pub fn busiest<'a>(samples: impl IntoIterator<Item = (&'a str, f64)>) -> f64 {
    let mut per_engine: HashMap<&str, f64> = HashMap::new();
    for (name, value) in samples {
        if let Some(engine) = engine_of(name)
            && value.is_finite()
            && value > 0.0
        {
            *per_engine.entry(engine).or_insert(0.0) += value;
        }
    }
    per_engine.into_values().fold(0.0, f64::max).min(100.0)
}

#[cfg(windows)]
pub use win::Counters;

#[cfg(windows)]
mod win {
    use super::busiest;
    use windows_sys::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
        PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };

    const PATH: &str = "\\GPU Engine(*)\\Utilization Percentage";

    /// An open counter query. Lives on the thread that reads it.
    pub struct Counters {
        query: *mut core::ffi::c_void,
        counter: *mut core::ffi::c_void,
        /// u64 words, so the items PDH writes into it are aligned
        buf: Vec<u64>,
    }

    impl Counters {
        /// None when this Windows has no GPU counters (older than Windows 10 1709, or a
        /// display driver that doesn't provide them).
        pub fn open() -> Option<Self> {
            let path: Vec<u16> = PATH.encode_utf16().chain([0]).collect();
            let mut query = std::ptr::null_mut();
            let mut counter = std::ptr::null_mut();
            // SAFETY: plain PDH calls with pointers to locals; the query is closed on failure.
            unsafe {
                if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                    return None;
                }
                if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                    PdhCloseQuery(query);
                    return None;
                }
                PdhCollectQueryData(query); // the first sample: a rate needs two
            }
            Some(Counters { query, counter, buf: Vec::new() })
        }

        /// The busiest GPU engine's load since the last call, in percent. None if Windows
        /// couldn't give a reading this time.
        pub fn read(&mut self) -> Option<u8> {
            // SAFETY: `buf` is sized to what PDH asked for before it is written; the names in
            // the items point into `buf` and are only read while it is alive and unchanged.
            unsafe {
                if PdhCollectQueryData(self.query) != 0 {
                    return None;
                }
                let (mut size, mut count) = (0u32, 0u32);
                let mut status = PdhGetFormattedCounterArrayW(
                    self.counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    std::ptr::null_mut(),
                );
                // the instance list can grow between the two calls: ask again if it did
                for _ in 0..3 {
                    if status != PDH_MORE_DATA {
                        break;
                    }
                    self.buf.resize((size as usize).div_ceil(8), 0);
                    size = (self.buf.len() * 8) as u32;
                    status = PdhGetFormattedCounterArrayW(
                        self.counter,
                        PDH_FMT_DOUBLE,
                        &mut size,
                        &mut count,
                        self.buf.as_mut_ptr().cast(),
                    );
                }
                if status != 0 {
                    return None;
                }
                let items = std::slice::from_raw_parts(
                    self.buf.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                    count as usize,
                );
                let names: Vec<String> = items.iter().map(|it| wide_to_string(it.szName)).collect();
                let load = busiest(
                    items
                        .iter()
                        .zip(&names)
                        .filter(|(it, _)| it.FmtValue.CStatus == 0)
                        .map(|(it, name)| (name.as_str(), it.FmtValue.Anonymous.doubleValue)),
                );
                Some(load as u8)
            }
        }
    }

    impl Drop for Counters {
        fn drop(&mut self) {
            // SAFETY: the query handle came from PdhOpenQueryW and is closed once.
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    /// A NUL-terminated UTF-16 string from PDH.
    unsafe fn wide_to_string(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut len = 0;
        // SAFETY (caller): `p` points at a NUL-terminated string inside the PDH buffer.
        unsafe {
            while *p.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_is_the_sum_of_its_processes_and_the_pc_is_its_busiest_engine() {
        let samples = [
            ("pid_100_luid_0x0_0xAAAA_phys_0_eng_0_engtype_3D", 20.0),
            ("pid_200_luid_0x0_0xAAAA_phys_0_eng_0_engtype_3D", 15.5),
            ("pid_100_luid_0x0_0xAAAA_phys_0_eng_3_engtype_VideoDecode", 30.0),
            ("pid_300_luid_0x0_0xBBBB_phys_0_eng_0_engtype_3D", 5.0), // the other GPU
            ("pid_300_luid_0x0_0xBBBB_phys_0_eng_1_engtype_Copy", 0.0),
        ];
        assert_eq!(busiest(samples), 35.5);
    }

    #[test]
    fn odd_input_is_ignored_and_the_result_stays_in_range() {
        assert_eq!(busiest([]), 0.0);
        assert_eq!(busiest([("_Total", 50.0), ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", f64::NAN)]), 0.0);
        let hot = [
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 80.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 70.0),
        ];
        assert_eq!(busiest(hot), 100.0);
    }

    #[cfg(windows)]
    #[test]
    fn windows_gives_a_reading_when_it_has_gpu_counters() {
        // machines without the counters (CI runners can lack a GPU) simply don't open
        if let Some(mut c) = Counters::open() {
            std::thread::sleep(std::time::Duration::from_millis(300));
            if let Some(v) = c.read() {
                println!("gpu {v}%");
                assert!(v <= 100);
            }
        }
    }
}
