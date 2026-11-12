use super::*;
use mach2::bootstrap::BOOTSTRAP_UNKNOWN_SERVICE;
use mach2::mach_port::{mach_port_allocate, mach_port_insert_right};
use mach2::message::MACH_MSG_TYPE_MAKE_SEND;
use std::time::{Duration, Instant};

/// A port of this task with one send right, standing in for a workgroup's port.
fn send_right() -> mach_port_t {
    let mut port = MACH_PORT_NULL;
    // SAFETY: `port` is a writable out-parameter for a new receive right in this task.
    let allocated = unsafe { mach_port_allocate(this_task(), MACH_PORT_RIGHT_RECEIVE, &mut port) };
    assert_eq!(allocated, 0, "mach_port_allocate");
    // SAFETY: this task holds the receive right for `port`, so it can make a send right.
    let inserted =
        unsafe { mach_port_insert_right(this_task(), port, port, MACH_MSG_TYPE_MAKE_SEND) };
    assert_eq!(inserted, 0, "mach_port_insert_right");
    port
}

fn look_up(name: &CString) -> i32 {
    let mut port = MACH_PORT_NULL;
    // SAFETY: `name` is a C string and `bootstrap_port` belongs to this process.
    let status = unsafe { bootstrap_look_up(bootstrap_port, name.as_ptr(), &mut port) };
    deallocate_port(port);
    status
}

/// The helper releases the right it checked in once the port has arrived, so the bootstrap
/// name does not stay registered, and its receive right held, for the helper's lifetime.
#[test]
fn a_received_workgroup_port_releases_the_checked_in_name() {
    let name = service_name();
    let joiner = {
        let name = name.clone();
        // A zero period fails after the port is received, before any workgroup exists. A
        // membership stays on its thread, so only the outcome comes back.
        std::thread::spawn(move || {
            matches!(
                join_passed_workgroup(&name, 0),
                Err(WorkgroupError::ThreadPolicy { period_ticks: 0 })
            )
        })
    };
    let port = send_right();
    send_workgroup_port(&name, port).expect("send the port to the checked-in name");
    assert!(
        joiner.join().expect("joiner"),
        "the join did not stop at the thread policy"
    );

    let name = CString::new(name).expect("service name");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut status = look_up(&name);
    while status != BOOTSTRAP_UNKNOWN_SERVICE && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        status = look_up(&name);
    }
    deallocate_port(port);
    release_receive_right(port);
    assert_eq!(
        status, BOOTSTRAP_UNKNOWN_SERVICE,
        "the checked-in name stayed registered after the handoff"
    );
}
