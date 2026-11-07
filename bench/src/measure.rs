//! timing a child process and describing the machine it ran on.

use std::io;
use std::process::{Command, Output};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::contract::Text;
use crate::record::Machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub started_at_unix_ms: u64,
    pub wall_time_ns: u64,
    pub cpu_user_ns: u64,
    pub cpu_system_ns: u64,
}

/// runs `command` to completion and measures it. CPU time comes from the difference in
/// `RUSAGE_CHILDREN` across the run, which counts the child and every descendant it waited for, so
/// rustc processes under Cargo are included. the difference is only this child's while no other
/// thread in the process reaps a child during the run, so callers measure from one thread.
pub fn run_measured(command: &mut Command) -> io::Result<(Measurement, Output)> {
    let before = children_rusage()?;
    let started_at = SystemTime::now();
    let start = Instant::now();
    let output = command.output()?;
    let wall = start.elapsed();
    let after = children_rusage()?;
    let measurement = Measurement {
        started_at_unix_ms: started_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| saturating_u64(d.as_millis())),
        wall_time_ns: saturating_u64(wall.as_nanos()),
        cpu_user_ns: saturating_u64(after.user.saturating_sub(before.user).as_nanos()),
        cpu_system_ns: saturating_u64(after.system.saturating_sub(before.system).as_nanos()),
    };
    Ok((measurement, output))
}

struct CpuTimes {
    user: Duration,
    system: Duration,
}

fn children_rusage() -> io::Result<CpuTimes> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage writes a complete rusage into the pointer it's given and reads nothing from
    // it; the buffer is zeroed, properly aligned and lives for the call.
    let status = unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: getrusage returned 0, so it initialised the struct.
    let usage = unsafe { usage.assume_init() };
    let duration = |t: libc::timeval| {
        Duration::from_secs(u64::try_from(t.tv_sec).unwrap_or(0))
            + Duration::from_micros(u64::try_from(t.tv_usec).unwrap_or(0))
    };
    Ok(CpuTimes {
        user: duration(usage.ru_utime),
        system: duration(usage.ru_stime),
    })
}

fn saturating_u64(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

/// describes this machine. `name` overrides the host name, which is what a published result should
/// carry when the host name itself is private.
pub fn machine(name: Option<Text>) -> io::Result<Machine> {
    let name = match name {
        Some(name) => name,
        None => Text::new(hostname()?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
    };
    let logical_cpus = std::thread::available_parallelism()
        .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX));
    Ok(Machine {
        name,
        cpu_model: cpu_model(),
        logical_cpus,
        os: Text::new(std::env::consts::OS).expect("the target os name is never empty"),
        arch: Text::new(std::env::consts::ARCH).expect("the target arch name is never empty"),
    })
}

fn hostname() -> io::Result<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: gethostname writes at most buffer.len() bytes into the buffer it's given.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    Ok(String::from_utf8_lossy(&buffer[..end]).into_owned())
}

/// Linux reports the model in /proc/cpuinfo; elsewhere the field is null rather than a guess.
fn cpu_model() -> Option<Text> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name")?.split_once(':'))
        .and_then(|(_, model)| Text::new(model.trim()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_cpu_time_of_the_child() {
        // a busy loop in a child shell; its CPU time must show up even though this process idles.
        let mut command = Command::new("sh");
        command.args(["-c", "i=0; while [ $i -lt 200000 ]; do i=$((i+1)); done"]);
        let (measurement, output) = run_measured(&mut command).unwrap();
        assert!(output.status.success());
        assert!(measurement.cpu_user_ns + measurement.cpu_system_ns > 0);
        assert!(measurement.wall_time_ns > 0);
    }
}
