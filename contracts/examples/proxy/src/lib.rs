//! Calls other contracts on behalf of whoever calls it.

#![no_std]

extern crate alloc;

use chainv4_sdk::{Value, abort, call, export, input, output};

/// forward(target, function, args...) calls target's function with the rest of the arguments
/// and returns what it returns.
#[export]
fn forward() {
    let mut args = input();

    if args.len() < 2 {
        abort("forward takes a target, a function name and its arguments");
    }

    let rest = args.split_off(2);
    let (Some(target), Some(function)) = (args[0].as_address(), args[1].as_str()) else {
        abort("forward takes a target address and a function name");
    };

    match call(&target, function, &rest) {
        Ok(returns) => output(&returns),
        Err(err) => abort(&err),
    }
}

/// Answers callbacks. Marked reentrant, so a contract this proxy is calling can call back into
/// it; forward is not, so nothing can make the proxy forward twice at once.
#[export(reentrant)]
fn ping() {
    output(&[Value::String("pong".into())]);
}
