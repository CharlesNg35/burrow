use wasmparser::{Validator, WasmFeatures};

pub const PROBE: [u8; 31] = [
    0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 0, 1, 123, 3, 2, 1, 0, 10, 10, 1, 8, 0, 65, 0, 253,
    15, 253, 98, 11,
];

pub const TOO_OLD: &str = "burrow admin needs a newer browser: Chrome or Edge 91 or later, Firefox 89 or later, or Safari 16.4 or later.";

fn probe_js() -> String {
    let bytes: Vec<String> = PROBE.iter().map(u8::to_string).collect();
    format!(
        "typeof WebAssembly === \"object\" && WebAssembly.validate(new Uint8Array([{}]))",
        bytes.join(",")
    )
}

pub fn boot_js(js: &str, wasm: &str) -> String {
    format!(
        "import init from \"./{js}\";\nif ({}) {{\n  init({{ module_or_path: new URL(\"./{wasm}\", import.meta.url) }});\n}} else {{\n  const p = document.getElementById(\"boot\");\n  p.textContent = \"{TOO_OLD}\";\n  p.setAttribute(\"role\", \"alert\");\n  p.dataset.unsupported = \"simd\";\n}}\n",
        probe_js()
    )
}

pub fn uses_simd(wasm: &[u8]) -> Result<bool, String> {
    Validator::new()
        .validate_all(wasm)
        .map_err(|e| format!("a built wasm does not validate: {e}"))?;
    let plain = WasmFeatures::default() - WasmFeatures::SIMD - WasmFeatures::RELAXED_SIMD;
    Ok(Validator::new_with_features(plain)
        .validate_all(wasm)
        .is_err())
}

pub fn guard(boot: &str, wasms: &[&[u8]]) -> Result<(), String> {
    for w in wasms {
        if uses_simd(w)? && !boot.contains(&probe_js()) {
            return Err(
                "a wasm uses WebAssembly SIMD, but boot.js does not check for it first; a browser without SIMD would show a blank page".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, Function, FunctionSection, Instruction, Module, TypeSection, ValType,
    };

    fn module(body: &[Instruction<'_>], result: ValType) -> Vec<u8> {
        let mut types = TypeSection::new();
        types.ty().function([], [result]);
        let mut funcs = FunctionSection::new();
        funcs.function(0);
        let mut f = Function::new(Vec::<(u32, ValType)>::new());
        for i in body {
            f.instruction(i);
        }
        f.instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&f);
        let mut m = Module::new();
        m.section(&types).section(&funcs).section(&code);
        m.finish()
    }

    fn plain() -> Vec<u8> {
        module(&[Instruction::I32Const(1)], ValType::I32)
    }

    fn simd() -> Vec<u8> {
        module(
            &[
                Instruction::I32Const(3),
                Instruction::I32x4Splat,
                Instruction::I32x4ExtractLane(0),
            ],
            ValType::I32,
        )
    }

    #[test]
    fn simd_is_found_in_a_body_and_in_a_type() {
        assert_eq!(uses_simd(&plain()), Ok(false));
        assert_eq!(uses_simd(&simd()), Ok(true));
        let v128 = module(&[Instruction::V128Const(7)], ValType::V128);
        assert_eq!(uses_simd(&v128), Ok(true));
        assert!(uses_simd(b"not wasm").is_err());
    }

    #[test]
    fn the_probe_is_a_valid_module_that_needs_simd() {
        assert_eq!(uses_simd(&PROBE), Ok(true));
    }

    #[test]
    fn a_simd_wasm_ships_only_with_the_boot_check() {
        let boot = boot_js("app-1.js", "app-2.wasm");
        assert!(boot.contains(&probe_js()));
        assert!(boot.contains(TOO_OLD));
        assert!(boot.contains("new URL(\"./app-2.wasm\", import.meta.url)"));
        assert!(boot.starts_with("import init from \"./app-1.js\";"));
        let (p, s) = (plain(), simd());
        assert_eq!(guard(&boot, &[p.as_slice(), s.as_slice()]), Ok(()));
        let unchecked = "import init from \"./app.js\";\ninit();\n";
        assert_eq!(guard(unchecked, &[p.as_slice()]), Ok(()));
        let e = guard(unchecked, &[p.as_slice(), s.as_slice()]).expect_err("refused");
        assert!(e.contains("boot.js does not check for it first"), "{e}");
    }
}
