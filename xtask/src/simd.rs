use wasmparser::{Validator, WasmFeatures};

pub fn uses_simd(wasm: &[u8]) -> Result<bool, String> {
    Validator::new()
        .validate_all(wasm)
        .map_err(|e| format!("a built wasm does not validate: {e}"))?;
    let plain = WasmFeatures::default() - WasmFeatures::SIMD - WasmFeatures::RELAXED_SIMD;
    Ok(Validator::new_with_features(plain)
        .validate_all(wasm)
        .is_err())
}

pub fn refuse(wasms: &[(&str, &[u8])]) -> Result<(), String> {
    for (name, wasm) in wasms {
        if uses_simd(wasm)? {
            return Err(format!(
                "{name} uses WebAssembly SIMD, which browsers before Safari 16.4 cannot run; find the crate that enables simd128 (docs/decisions.md D88)"
            ));
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
    fn a_wasm_with_simd_is_refused_by_name() {
        let (p, s) = (plain(), simd());
        assert_eq!(
            refuse(&[("app.wasm", p.as_slice()), ("split_x.wasm", p.as_slice())]),
            Ok(())
        );
        assert_eq!(
            refuse(&[("app.wasm", p.as_slice()), ("split_x.wasm", s.as_slice())]),
            Err("split_x.wasm uses WebAssembly SIMD, which browsers before Safari 16.4 cannot run; find the crate that enables simd128 (docs/decisions.md D88)".into())
        );
        assert!(refuse(&[("app.wasm", b"not wasm".as_slice())]).is_err());
    }
}
