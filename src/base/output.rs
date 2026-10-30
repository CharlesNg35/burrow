use std::io::Write;

use super::failure::Failure;

pub fn say(out: &mut dyn Write, line: impl AsRef<str>) -> Result<(), Failure> {
    put(out, &format!("{}\n", line.as_ref()))
}

pub fn put(out: &mut dyn Write, text: &str) -> Result<(), Failure> {
    match out.write_all(text.as_bytes()).and_then(|_| out.flush()) {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(Failure::new(format!("cannot write output: {e}")))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fails(std::io::ErrorKind);

    impl Write for Fails {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(self.0.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_pipe_is_fine_and_other_write_errors_fail() {
        assert!(say(&mut Fails(std::io::ErrorKind::BrokenPipe), "x").is_ok());
        let full = say(&mut Fails(std::io::ErrorKind::StorageFull), "x").unwrap_err();
        assert!(
            full.message.starts_with("cannot write output: "),
            "{}",
            full.message
        );
        let mut got = Vec::new();
        say(&mut got, "hello").unwrap();
        put(&mut got, "a").unwrap();
        assert_eq!(got, b"hello\na");
    }
}
