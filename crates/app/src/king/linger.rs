//! What an agent's turn left running.
//!
//! An agent may start a program that outlives its answer: a development
//! server, a watcher (Claude Code's background commands are such). Its
//! process then does not end with the turn, and the turn is over all the
//! same once the agent said its result. The service keeps such a process,
//! and lists what runs under it: the programs, not the agent's own servers
//! of tools nor the shells that wrap a command, so the person sees what was
//! started and can stop it.
//!
//! [`left_running`] reads the process table as `ps -A -o pid=,ppid=,args=`
//! prints it.

/// The programs running under `root`: its descendants that are not servers
/// of tools (their command names MCP) and that run nothing themselves (a
/// shell that wraps a command is its parent), as id and command line.
pub fn left_running(table: &str, root: u32) -> Vec<(u32, String)> {
    let rows: Vec<(u32, u32, &str)> = table
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ppid = words.next()?.parse().ok()?;
            let rest = line
                .trim_start()
                .splitn(3, char::is_whitespace)
                .nth(2)
                .unwrap_or_default()
                .trim();
            Some((pid, ppid, rest))
        })
        .collect();
    let mut under = vec![root];
    let mut found: Vec<(u32, u32, &str)> = Vec::new();
    while let Some(parent) = under.pop() {
        for row in rows.iter().filter(|row| row.1 == parent) {
            if !found.iter().any(|seen| seen.0 == row.0) {
                found.push(*row);
                under.push(row.0);
            }
        }
    }
    let tools = |args: &str| {
        args.split_whitespace()
            .any(|word| word == "mcp" || word.ends_with("-mcp"))
    };
    found
        .iter()
        .filter(|row| !tools(row.2))
        .filter(|row| {
            !found
                .iter()
                .any(|child| child.1 == row.0 && !tools(child.2))
        })
        .map(|row| (row.0, row.2.to_owned()))
        .collect()
}

/// The process table of this computer, as [`left_running`] reads it.
pub fn process_table() -> Option<String> {
    let output = leon_remote::spawn::std_child("ps")
        .args(["-A", "-o", "pid=,ppid=,args="])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Asks a process of this computer to end.
pub fn terminate(pid: u32) {
    #[cfg(unix)]
    // SAFETY: `kill` takes plain integers; the pid is one the service found
    // under a process it started itself.
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_programs_a_turn_left_are_listed_without_its_tools_and_shells() {
        let table = "\
            1 0 /sbin/init\n\
            100 1 claude -p --input-format stream-json\n\
            101 100 /home/me/leon --data-dir /d king mcp\n\
            102 100 /home/me/leon --data-dir /d task mcp --anywhere\n\
            103 100 /usr/bin/bash -c source snapshot.sh && python3 app/server.py --port 8790\n\
            104 103 python3 app/server.py --port 8790\n\
            105 100 node /x/codegraph-mcp\n\
            200 1 python3 app/server.py --port 8787\n";
        assert_eq!(
            left_running(table, 100),
            [(104, "python3 app/server.py --port 8790".to_owned())]
        );
        // Nothing under a process that left nothing.
        assert!(left_running(table, 104).is_empty());
        assert!(left_running("", 100).is_empty());
    }
}
