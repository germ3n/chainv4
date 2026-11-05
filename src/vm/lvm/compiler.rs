//! Turns contract source into Luau bytecode. Every setting here is part of consensus: the
//! bytecode, and with it the fuel each call costs, depends on them, so every node must compile
//! the same way. Contracts are deployed as source for the same reason, and because Luau does not
//! check bytecode: loading bytecode someone else made can corrupt memory.

use {
    crate::vm::VmError,
    mlua::chunk::Compiler,
    r#macro::document,
};

/// Luau's default: constant folding and other local optimizations, no inlining across functions.
pub const OPTIMIZATION_LEVEL: u8 = 1;
/// Line numbers, so an error says where it happened.
pub const DEBUG_LEVEL: u8 = 1;
/// None. Type information only feeds native code generation, which contracts never use.
pub const TYPE_INFO_LEVEL: u8 = 0;
/// The largest source a contract may have by default: a block's contract_limits.source_len may
/// say otherwise. Compiling is not metered by fuel, so this bounds the work one deploy can cause.
pub const MAX_SOURCE_LEN: usize = crate::params::vm::ContractLimits::DEFAULT.source_len as usize;

/// The math functions Luau's compiler knows as builtins: it folds a call to one whose arguments
/// are constants, through the C library, and calls one directly where it can. Those scripts do not
/// get (see `MATH`) are turned off, so a call to one compiles to a lookup that finds nothing, as it
/// does at run time, and no bytecode depends on how a platform's C library rounds.
const MATH_BUILTINS: &[&str] = &[
    "abs", "acos", "asin", "atan2", "atan", "ceil", "cosh", "cos", "deg", "exp", "floor", "fmod",
    "frexp", "ldexp", "log10", "log", "max", "min", "modf", "pow", "rad", "sinh", "sin", "sqrt",
    "tanh", "tan", "clamp", "sign", "round", "lerp", "isnan", "isinf", "isfinite",
];

fn disabled_builtins() -> impl Iterator<Item = String> {
    MATH_BUILTINS.iter().filter(|name| !super::MATH.contains(name)).map(|name| format!("math.{name}"))
}

#[document(
    summary = "Compiles contract source with the chain's fixed settings. The same source always gives the same bytecode.",
    category = "Lua VM",
    params = { source = "The Luau source." },
    returns = "The bytecode, or why it does not compile.",
    see_also = "Contract::from_bytecode",
)]
pub fn compile(source: &str) -> Result<Vec<u8>, VmError> {
    compile_within(source, MAX_SOURCE_LEN)
}

#[document(
    summary = "Compiles contract source as compile does, refusing source longer than a block's limit.",
    category = "Lua VM",
    params = { source = "The Luau source.", longest = "The most bytes it may take." },
    returns = "The bytecode, or why it does not compile.",
    see_also = "compile, ContractLimits",
)]
pub fn compile_within(source: &str, longest: usize) -> Result<Vec<u8>, VmError> {
    if source.len() > longest {
        return Err(VmError::Compile(format!(
            "source is {} bytes, more than the {longest} allowed",
            source.len()
        )));
    }

    Compiler::new()
        .set_optimization_level(OPTIMIZATION_LEVEL)
        .set_debug_level(DEBUG_LEVEL)
        .set_type_info_level(TYPE_INFO_LEVEL)
        .set_disabled_builtins(disabled_builtins())
        .compile(source)
        .map_err(|err| VmError::Compile(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_source_gives_the_same_bytecode() {
        let source = "local M = {} function M.f(a) return a * 2 end return M";

        assert_eq!(compile(source).unwrap(), compile(source).unwrap());
        assert_ne!(compile(source).unwrap(), compile("return {}").unwrap());
    }

    #[test]
    fn syntax_errors_and_huge_sources_are_rejected() {
        let huge = format!("return {{}} --{}", "x".repeat(MAX_SOURCE_LEN));

        assert!(matches!(compile("local = 1"), Err(VmError::Compile(_))));
        assert!(matches!(compile(&huge), Err(VmError::Compile(text)) if text.contains("allowed")));
    }
}
