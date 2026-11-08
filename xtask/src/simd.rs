use wasmparser::{Validator, WasmFeatures};

pub fn refuse_in_main(wasm: &[u8]) -> Result<(), String> {
    let features = WasmFeatures::default() - WasmFeatures::SIMD - WasmFeatures::RELAXED_SIMD;
    Validator::new_with_features(features)
        .validate_all(wasm)
        .map(|_| ())
        .map_err(|e| {
            format!(
                "the main wasm uses WebAssembly SIMD ({e}), so browsers without it could not open burrow admin; keep the code that needs SIMD behind a lazy chunk"
            )
        })
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

    #[test]
    fn a_main_wasm_with_simd_is_refused_and_one_without_passes() {
        let plain = module(&[Instruction::I32Const(1)], ValType::I32);
        assert_eq!(refuse_in_main(&plain), Ok(()));
        let simd = module(&[Instruction::V128Const(7)], ValType::V128);
        let e = refuse_in_main(&simd).expect_err("SIMD is refused");
        assert!(
            e.starts_with("the main wasm uses WebAssembly SIMD ("),
            "{e}"
        );
        let lane = module(
            &[
                Instruction::I32Const(3),
                Instruction::I32x4Splat,
                Instruction::I32x4ExtractLane(0),
            ],
            ValType::I32,
        );
        assert!(
            refuse_in_main(&lane).is_err(),
            "a SIMD op inside an i32 function too"
        );
    }
}
