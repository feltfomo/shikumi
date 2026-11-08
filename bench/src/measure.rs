//! the clocks a measurement reads: the monotonic clock steps are timed on, CPU time of a child
//! process tree or of the whole machine, and a description of the machine itself.

use std::io;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::contract::Text;
use crate::record::Machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Measurement {
    pub started_at_unix_ms: u64,
    /// the monotonic-clock reading the wall time starts from, which a timeline anchors on.
    pub start_ns: u64,
    pub wall_time_ns: u64,
    pub cpu_user_ns: u64,
    pub cpu_system_ns: u64,
}

/// nanoseconds on the monotonic clock. it's shared by every process on the machine, so a child can
/// report readings a parent's timeline accepts, which `Instant` can't carry across a process.
pub fn monotonic_ns() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes one timespec into the pointer, which points at a live local.
    // CLOCK_MONOTONIC exists on every platform the harness builds for, so it can't fail.
    let status = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    debug_assert_eq!(status, 0, "CLOCK_MONOTONIC is always readable");
    u64::try_from(now.tv_sec).unwrap_or(0) * 1_000_000_000 + u64::try_from(now.tv_nsec).unwrap_or(0)
}

/// milliseconds since the Unix epoch, for a record's start time.
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| saturating_u64(d.as_millis()))
}

/// user and system CPU time summed over every CPU of the machine, from `/proc/stat`. it's the
/// only CPU measure that sees work a daemon does on a client's behalf, so it's what a build
/// through the store is charged with; it includes whatever else the machine ran meanwhile.
pub fn machine_cpu() -> io::Result<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat")?;
    // SAFETY: sysconf only reads a configuration value.
    let per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    cpu_times(&stat, u64::try_from(per_second).unwrap_or(100).max(1))
}

/// user and system nanoseconds from `/proc/stat`'s first line: user and nice are user time,
/// system, irq and softirq system time, in ticks of `per_second`.
fn cpu_times(stat: &str, per_second: u64) -> io::Result<(u64, u64)> {
    let line = stat
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "/proc/stat has no cpu line"))?;
    let ticks: Vec<u64> = line
        .split_ascii_whitespace()
        .skip(1)
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    let field = |index: usize| ticks.get(index).copied().unwrap_or(0);
    let ns = |ticks: u64| ticks * (1_000_000_000 / per_second);
    Ok((ns(field(0) + field(1)), ns(field(2) + field(5) + field(6))))
}

/// runs `command` to completion and measures it. CPU time comes from the difference in
/// `RUSAGE_CHILDREN` across the run, which counts the child and every descendant it waited for, so
/// rustc processes under Cargo are included. the difference is only this child's while no other
/// thread in the process reaps a child during the run, so callers measure from one thread.
pub fn run_measured(command: &mut Command) -> io::Result<(Measurement, Output)> {
    let before = children_rusage()?;
    let started_at_unix_ms = unix_ms();
    let start_ns = monotonic_ns();
    let output = command.output()?;
    let wall_time_ns = monotonic_ns() - start_ns;
    let after = children_rusage()?;
    let measurement = Measurement {
        started_at_unix_ms,
        start_ns,
        wall_time_ns,
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

    // the fields `machine_cpu` charges: user and nice as user time; system, irq and softirq as
    // system time; idle, iowait and steal left out.
    #[test]
    fn cpu_times_sum_the_user_and_system_fields() {
        let stat = "cpu  10 2 30 400 50 6 7 80 0 0\ncpu0 1 1 1 1 1 1 1 1 0 0\n";
        let (user, system) = cpu_times(stat, 100).unwrap();
        assert_eq!(user, 12 * 10_000_000);
        assert_eq!(system, (30 + 6 + 7) * 10_000_000);
        assert!(cpu_times("intr 1 2 3\n", 100).is_err());
    }
}
