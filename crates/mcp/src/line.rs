//! Reading one NDJSON line with a cap on its length.
//!
//! The control socket takes connections from any process of the user.
//! `read_line` grows its buffer for as long as bytes come without a
//! newline, and fails on a line that isn't UTF-8, which closed the whole
//! connection without a reply (#288). `read_capped` stops at `max` bytes and
//! leaves the bytes to the caller to decode.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// What `read_capped` read.
#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    /// A line in the buffer, its newline included unless the input ended.
    Read,
    /// The input ended with nothing more.
    End,
    /// The line went over the cap; the caller should close the connection,
    /// as what follows can't be told apart from the rest of that line.
    TooLong,
}

/// Read up to and including the next newline into `buf`, which is cleared
/// first, reading at most `max` bytes.
pub async fn read_capped<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<Line> {
    buf.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(if buf.is_empty() {
                Line::End
            } else {
                Line::Read
            });
        }
        let (take, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        if buf.len() + take > max {
            reader.consume(take);
            return Ok(Line::TooLong);
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if done {
            return Ok(Line::Read);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    fn block_on<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn lines_come_one_at_a_time_and_the_last_may_lack_its_newline() {
        block_on(async {
            let mut r = BufReader::with_capacity(4, &b"{\"a\":1}\n{}\nlast"[..]);
            let mut buf = Vec::new();
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::Read);
            assert_eq!(buf, b"{\"a\":1}\n");
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::Read);
            assert_eq!(buf, b"{}\n");
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::Read);
            assert_eq!(buf, b"last");
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::End);
        });
    }

    /// A peer that sends bytes with no newline grew the buffer without end.
    #[test]
    fn a_line_over_the_cap_stops_the_read() {
        block_on(async {
            let endless = vec![b'x'; 10_000];
            let mut r = BufReader::with_capacity(512, &endless[..]);
            let mut buf = Vec::new();
            assert_eq!(
                read_capped(&mut r, &mut buf, 1_000).await.unwrap(),
                Line::TooLong
            );
            assert!(buf.len() <= 1_000);
        });
    }

    /// Invalid UTF-8 is the caller's to judge; it no longer fails the read.
    #[test]
    fn bytes_that_are_not_utf8_are_still_a_line() {
        block_on(async {
            let mut r = BufReader::new(&b"\xff\xfe\n{}\n"[..]);
            let mut buf = Vec::new();
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::Read);
            assert_eq!(buf, b"\xff\xfe\n");
            assert_eq!(read_capped(&mut r, &mut buf, 64).await.unwrap(), Line::Read);
            assert_eq!(buf, b"{}\n");
        });
    }
}
