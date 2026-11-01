//! How many threads ONNX Runtime should run a model on.
//!
//! An inference operator is split evenly across its intra-op threads and
//! finishes when the slowest one does. On a hybrid processor (Apple M-series,
//! Intel 12th generation and later) a thread placed on an efficiency core
//! runs at a fraction of a performance core's speed, so using every core
//! makes each operator wait for the efficiency cores, and the model runs
//! slower on all of them than on its performance cores alone. Hyper-threads
//! share one
//! core's vector units, which the matrix kernels already saturate, so
//! physical cores are counted, not logical processors.
//!
//! This sets how many threads a session starts, not where they run: the
//! operating system still places them (a window in the background can be
//! given efficiency cores by Windows power throttling or macOS quality of
//! service). On AMD processors that mix full and compact cores (Zen 5 and
//! Zen 5c), Windows reports the compact cores as a lower class, so only the
//! full cores are counted, although compact cores are not much slower.
//! None of this has been measured on a hybrid machine yet.

/// Intra-op threads for an inference session that runs on the CPU: the
/// performance cores of this machine (physical cores of the fastest class),
/// at least one. Falls back to the logical processors when the platform does
/// not say.
pub fn inference_threads() -> usize {
    platform::performance_cores()
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
        .max(1)
}

/// Cores of the fastest class among `classes` (one efficiency class per
/// physical core, higher is faster, as Windows reports them). A processor
/// that is not hybrid reports one class for every core: all of them count.
pub fn performance_cores(classes: &[u8]) -> usize {
    match classes.iter().max() {
        Some(top) => classes.iter().filter(|&&c| c == *top).count(),
        None => 0,
    }
}

/// The efficiency class of each physical core from the buffer
/// `GetLogicalProcessorInformationEx(RelationProcessorCore, ...)` fills: a
/// sequence of variable-length `SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX`
/// records (`Relationship: u32`, `Size: u32`, then for a core the
/// `PROCESSOR_RELATIONSHIP`: `Flags: u8`, `EfficiencyClass: u8`, ...). Records
/// of another relationship are skipped; a truncated or malformed record ends
/// the list.
pub fn efficiency_classes(buffer: &[u8]) -> Vec<u8> {
    const RELATION_PROCESSOR_CORE: u32 = 0;
    const HEADER: usize = 8;
    let mut classes = Vec::new();
    let mut offset = 0usize;
    while offset + HEADER <= buffer.len() {
        let field = |at: usize| {
            u32::from_le_bytes([buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]])
        };
        let relationship = field(offset);
        let size = field(offset + 4) as usize;
        if size < HEADER + 2 || offset + size > buffer.len() {
            break;
        }
        if relationship == RELATION_PROCESSOR_CORE {
            classes.push(buffer[offset + HEADER + 1]);
        }
        offset += size;
    }
    classes
}

#[cfg(windows)]
mod platform {
    const RELATION_PROCESSOR_CORE: i32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLogicalProcessorInformationEx(
            relationship: i32,
            buffer: *mut u8,
            returned_length: *mut u32,
        ) -> i32;
    }

    pub fn performance_cores() -> Option<usize> {
        let mut length = 0u32;
        // SAFETY: a null buffer with length 0 only asks for the size needed.
        let first = unsafe {
            GetLogicalProcessorInformationEx(
                RELATION_PROCESSOR_CORE,
                std::ptr::null_mut(),
                &mut length,
            )
        };
        // SAFETY: reads the calling thread's last error.
        let last_error = unsafe { windows::Win32::Foundation::GetLastError() }.0;
        if first != 0 || last_error != ERROR_INSUFFICIENT_BUFFER || length == 0 {
            return None;
        }
        let mut buffer = vec![0u8; length as usize];
        // SAFETY: `buffer` holds `length` writable bytes, as the call requires.
        let filled = unsafe {
            GetLogicalProcessorInformationEx(
                RELATION_PROCESSOR_CORE,
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        if filled == 0 {
            return None;
        }
        buffer.truncate(length as usize);
        let cores = super::performance_cores(&super::efficiency_classes(&buffer));
        (cores > 0).then_some(cores)
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::CStr;

    fn sysctl_u32(name: &CStr) -> Option<u32> {
        let mut value: u32 = 0;
        let mut size = std::mem::size_of::<u32>();
        // SAFETY: `value` is a writable u32 and `size` says so; the name is a
        // NUL-terminated string; no new value is set.
        let status = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                (&mut value as *mut u32).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0 && size == std::mem::size_of::<u32>()).then_some(value)
    }

    /// Performance level 0 is the fastest cores (on Apple silicon, the
    /// performance cores); Intel Macs have only one level. Older systems
    /// without performance levels report physical cores.
    pub fn performance_cores() -> Option<usize> {
        sysctl_u32(c"hw.perflevel0.physicalcpu")
            .or_else(|| sysctl_u32(c"hw.physicalcpu"))
            .map(|n| n as usize)
            .filter(|&n| n > 0)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    /// Not read on other platforms: the logical processors are used.
    pub fn performance_cores() -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One `RelationProcessorCore` record as Windows lays it out: header,
    /// flags, efficiency class, reserved bytes, group count and one group mask.
    fn core_record(class: u8) -> Vec<u8> {
        let size: u32 = 48;
        let mut record = Vec::with_capacity(size as usize);
        record.extend_from_slice(&0u32.to_le_bytes());
        record.extend_from_slice(&size.to_le_bytes());
        record.push(1); // LTP_PC_SMT
        record.push(class);
        record.extend_from_slice(&[0u8; 20]);
        record.extend_from_slice(&1u16.to_le_bytes());
        record.resize(size as usize, 0);
        record
    }

    #[test]
    fn a_hybrid_processor_counts_only_its_performance_cores() {
        // 12 performance cores (class 1) and 6 efficiency cores (class 0).
        let mut classes = vec![1u8; 12];
        classes.extend([0u8; 6]);
        assert_eq!(performance_cores(&classes), 12);
        // Intel's numbering can put the efficiency cores first.
        let mut reversed = vec![0u8; 8];
        reversed.extend([1u8; 6]);
        assert_eq!(performance_cores(&reversed), 6);
    }

    #[test]
    fn a_processor_with_one_class_counts_every_core() {
        assert_eq!(performance_cores(&[0, 0, 0, 0]), 4);
        assert_eq!(performance_cores(&[]), 0);
    }

    #[test]
    fn efficiency_classes_are_read_from_each_core_record() {
        let mut buffer = Vec::new();
        for class in [1, 1, 0, 1, 0] {
            buffer.extend(core_record(class));
        }
        assert_eq!(efficiency_classes(&buffer), [1, 1, 0, 1, 0]);
        assert_eq!(performance_cores(&efficiency_classes(&buffer)), 3);
    }

    #[test]
    fn records_of_other_relationships_are_skipped() {
        let mut buffer = core_record(2);
        let mut cache = core_record(9);
        cache[0] = 2; // RelationCache
        buffer.extend(cache);
        buffer.extend(core_record(2));
        assert_eq!(efficiency_classes(&buffer), [2, 2]);
    }

    #[test]
    fn a_truncated_buffer_ends_the_list_without_reading_past_it() {
        let mut buffer = core_record(1);
        let second = core_record(0);
        buffer.extend(&second[..20]);
        assert_eq!(efficiency_classes(&buffer), [1]);
        // A record claiming a size below its header is malformed.
        let mut bad = core_record(1);
        bad[4..8].copy_from_slice(&4u32.to_le_bytes());
        assert!(efficiency_classes(&bad).is_empty());
    }

    #[test]
    fn this_machine_gets_at_least_one_thread() {
        let threads = inference_threads();
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        assert!(
            threads >= 1 && threads <= logical.max(1),
            "{threads} of {logical}"
        );
    }
}
