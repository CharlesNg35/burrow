use std::io;

pub fn gone_before_accept(e: &io::Error) -> bool {
    use io::ErrorKind as K;
    matches!(
        e.kind(),
        K::ConnectionAborted | K::ConnectionReset | K::ConnectionRefused | K::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_lost_in_the_queue_is_skipped_but_running_out_of_files_is_not() {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
        ] {
            assert!(gone_before_accept(&kind.into()), "{kind:?}");
        }
        #[cfg(unix)]
        assert!(!gone_before_accept(&io::Error::from_raw_os_error(24)));
        assert!(!gone_before_accept(&io::ErrorKind::OutOfMemory.into()));
    }
}
