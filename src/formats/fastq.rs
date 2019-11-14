use std::cmp::min;

use memchr::memchr;

use crate::formats::buffer::RecParser;
use crate::formats::fasta::check_end;
use crate::sequence::Sequence;
use crate::sequence_record::SequenceRecord;
use crate::util::{memchr_both, ParseError, ParseErrorType};

/// A zero-copy reference to a FASTQ record in a buffer.
#[derive(Debug)]
pub struct FastqRecord<'a> {
    pub id: &'a [u8],
    pub seq: &'a [u8],
    pub id2: &'a [u8],
    pub qual: &'a [u8],
}

impl<'a> Sequence<'a> for FastqRecord<'a> {
    fn sequence(&self) -> &'a [u8] {
        self.seq
    }
}

impl<'a> From<FastqRecord<'a>> for SequenceRecord<'a> {
    fn from(fastq: FastqRecord<'a>) -> SequenceRecord<'a> {
        SequenceRecord::new(fastq.id.into(), fastq.seq.into(), Some(fastq.qual.into()))
    }
}

/// An iterator that parses a buffer into a sequence of FASTQRecords
pub struct FastqParser<'a> {
    buf: &'a [u8],
    last: bool,
    pos: usize,
}

impl<'a> FastqParser<'a> {
    pub fn new(buf: &'a [u8], last: bool) -> Result<Self, ParseError> {
        if buf[0] != b'@' {
            // sometimes there are extra returns at the end of a file so we shouldn't blow up
            if !(last && (buf[0] == b'\r' && buf[0] == b'\n')) {
                let context = String::from_utf8_lossy(&buf[..min(64, buf.len())]);
                let e = ParseError::new(
                    "FASTQ record must start with '@'",
                    ParseErrorType::InvalidHeader,
                )
                .context(context);
                return Err(e);
            }
        }

        Ok(FastqParser { buf, last, pos: 0 })
    }
}

impl<'a> Iterator for FastqParser<'a> {
    type Item = Result<FastqRecord<'a>, ParseError>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let buf = &self.buf[self.pos..];
        if buf.is_empty() {
            return None;
        }
        if buf[0] == b'\n' {
            // sometimes the last "record" is just newlines
            return None;
        }

        let id_end;
        match memchr(b'\n', &buf) {
            Some(i) => id_end = i + 1,
            None => return None,
        };
        let mut id = &buf[1..id_end - 1];

        let seq_end;
        match memchr_both(b'\n', b'+', &buf[id_end..]) {
            Some(i) => seq_end = id_end + i + 1,
            None => return None,
        };
        let mut seq = &buf[id_end..seq_end - 1];

        let id2_end;
        match memchr(b'\n', &buf[seq_end..]) {
            Some(i) => id2_end = seq_end + i + 1,
            None => return None,
        };
        let id2 = &buf[seq_end..id2_end - 1];

        // we know the qual scores must be the same length as the sequence
        // so we can just do some arithmatic instead of memchr'ing
        let mut qual_end = id2_end + seq.len() + 1;
        let mut buffer_used = qual_end;
        if qual_end > buf.len() {
            if !self.last {
                // we need to pull more into the buffer
                return None;
            }
            // now do some math to figure out if the file doesn't end with a newline
            let windows_ending = if seq.last() == Some(&b'\r') { 1 } else { 0 };
            if qual_end != buf.len() + 1 + windows_ending {
                return None;
            }
            buffer_used -= 1 + windows_ending;
            qual_end -= windows_ending;
        }
        let mut qual = &buf[id2_end..qual_end - 1];

        if (qual_end + 1 < buf.len()
            && buf[qual_end] != b'@'
            && buf[qual_end] != b'\r'
            && buf[qual_end] != b'\n')
            || (qual_end < buf.len() && buf[qual_end - 1] != b'\n')
        {
            let context = String::from_utf8_lossy(id);
            return Some(Err(ParseError::new(
                "Sequence and quality lengths differed",
                ParseErrorType::InvalidRecord,
            )
            .context(context)));
        }

        // clean up any extra '\r' from the id and seq
        if !id.is_empty() && id[id.len() - 1] == b'\r' {
            id = &id[..id.len() - 1];
        }
        if !seq.is_empty() && seq[seq.len() - 1] == b'\r' {
            seq = &seq[..seq.len() - 1];
        }
        // we do qual separately in case this is the end of the file
        if !qual.is_empty() && qual[qual.len() - 1] == b'\r' {
            qual = &qual[..qual.len() - 1];
        }
        if !qual.is_empty() && qual[qual.len() - 1] == b'\n' {
            // special case for FASTQs that are a single character short on the
            // quality line, but still have a terminal newline
            let context = String::from_utf8_lossy(id);
            return Some(Err(ParseError::new(
                "Quality length was shorter than expected",
                ParseErrorType::InvalidRecord,
            )
            .context(context)));
        }

        self.pos += buffer_used;
        Some(Ok(FastqRecord { id, seq, id2, qual }))
    }
}

impl<'a> RecParser<'a> for FastqParser<'a> {
    type Header = ();

    fn from_buffer(buf: &[u8], last: bool) -> FastqParser {
        FastqParser { buf, last, pos: 0 }
    }

    fn header(&mut self) -> Result<Self::Header, ParseError> {
        Ok(())
    }

    fn eof(&self) -> Result<(), ParseError> {
        check_end(&self.buf[self.pos..], self.last)
    }

    fn used(&self) -> usize {
        self.pos
    }
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use super::FastqParser;
    use crate::formats::buffer::{RecBuffer, RecParser};
    use crate::formats::parse_sequence_reader;
    use crate::util::ParseErrorType;

    fn seq(s: &[u8]) -> Cursor<&[u8]> {
        Cursor::new(&s[..])
    }

    #[test]
    fn test_fastq() {
        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@test\nAGCT\n+test\n~~a!\n@test2\nTGCA\n+test\nWUI9"),
            |_| (),
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"test");
                        assert_eq!(&seq.seq[..], b"AGCT");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~a!");
                    }
                    1 => {
                        assert_eq!(&seq.id[..], b"test2");
                        assert_eq!(&seq.seq[..], b"TGCA");
                        assert_eq!(&seq.qual.unwrap()[..], b"WUI9");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(i, 2);
        assert_eq!(res, Ok(()));

        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@test\r\nAGCT\r\n+test\r\n~~a!\r\n@test2\r\nTGCA\r\n+test\r\nWUI9"),
            |_| {},
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"test");
                        assert_eq!(&seq.seq[..], b"AGCT");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~a!");
                    }
                    1 => {
                        assert_eq!(&seq.id[..], b"test2");
                        assert_eq!(&seq.seq[..], b"TGCA");
                        assert_eq!(&seq.qual.unwrap()[..], b"WUI9");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(res, Ok(()));
        assert_eq!(i, 2);
    }

    #[test]
    fn test_fastq_endings() {
        //! Check for the absence of a panic. The parser previously assumed
        //! if the ID ended with an `\r\n` then the sequence did also.
        //! (Discovered via fuzzing)
        let res = parse_sequence_reader(seq(b"@\r\n\n+A\n@"), |_| (), |_seq| {});
        assert_eq!(res, Ok(()));
    }

    #[test]
    fn test_premature_endings() {
        let test = b"@test\nACGT\n+\nIII\n";
        let mut fp = FastqParser::new(test, true).unwrap();
        let result = fp.next().unwrap();
        assert!(result.is_err());
        let e = result.unwrap_err();
        // technically the terminal newline could be part of the record
        // so this is an InvalidRecord and not Invalid
        assert!(e.error_type == ParseErrorType::InvalidRecord);

        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@test\nAGCT\n+test\n~~a!\n@test2\nTGCA"),
            |_| {},
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"test");
                        assert_eq!(&seq.seq[..], b"AGCT");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~a!");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(i, 1);
        let e = res.unwrap_err();
        assert_eq!(e.error_type, ParseErrorType::Invalid);
        assert_eq!(e.record, 2);

        // we allow a few extra newlines at the ends of FASTQs
        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@test\nAGCT\n+test\n~~a!\n\n"),
            |_| {},
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"test");
                        assert_eq!(&seq.seq[..], b"AGCT");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~a!");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(i, 1);
        assert_eq!(res, Ok(()));

        // but if there's additional data past the newlines it's an error
        // note this is slightly easier to output than the "Sequence and
        // quality lengths differed" error because the end of the file may
        // normally have multiple newlines
        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@test\nAGCT\n+test\n~~a!\n\n@TEST\nA\n+TEST\n~"),
            |_| {},
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"test");
                        assert_eq!(&seq.seq[..], b"AGCT");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~a!");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(i, 1);
        let e = res.unwrap_err();
        assert_eq!(e.error_type, ParseErrorType::Invalid);
        assert_eq!(e.record, 2);
    }

    #[test]
    fn test_empty_records() {
        let mut i = 0;
        let res = parse_sequence_reader(
            seq(b"@\n\n+\n\n@test2\nTGCA\n+test2\n~~~~\n"),
            |stype| {
                assert_eq!(stype, "FASTQ");
            },
            |seq| {
                match i {
                    0 => {
                        assert_eq!(&seq.id[..], b"");
                        assert_eq!(&seq.seq[..], b"");
                        assert_eq!(&seq.qual.unwrap()[..], b"");
                    }
                    1 => {
                        assert_eq!(&seq.id[..], b"test2");
                        assert_eq!(&seq.seq[..], b"TGCA");
                        assert_eq!(&seq.qual.unwrap()[..], b"~~~~");
                    }
                    _ => unreachable!("Too many records"),
                }
                i += 1;
            },
        );
        assert_eq!(res, Ok(()));
        assert_eq!(i, 2);

        let test = b"@NCBI actually has files like this\nACGTACGATCGTACGTAGCTGCTAGCTAGCATGCATGACACACACGTACGATCGTACGTAGCTGCTAGCTAGCATGCATGACACAC\n+\n00000000000000000000000000000000000000000000000000000000000000000000000000000000000000\n@NCBI actually has files like this\n\n+\n\n@NCBI actually has files like this\nACGTACGATCGTACGTAGCTGCTAGCTAGCATGCATGACACACACGTACGATCGTACGTAGCTGCTAGCTAGCATGCATGACACAC\n+\n00000000000000000000000000000000000000000000000000000000000000000000000000000000000000";
        let mut fp = FastqParser::new(test, true).unwrap();
        let result = fp.next().unwrap();
        assert!(result.is_ok());
        let result = fp.next().unwrap();
        assert!(result.is_ok());
        let result = fp.next().unwrap();
        assert!(result.is_ok());

        let mut i = 0;
        let res = parse_sequence_reader(
            seq(test),
            |stype| assert_eq!(stype, "FASTQ"),
            |_| {
                i += 1;
            },
        );
        assert_eq!(i, 3);
        assert_eq!(res, Ok(()));
    }

    #[test]
    fn test_mismatched_lengths() {
        let mut fp = FastqParser::new(b"@test\nAGCT\n+\nIII\n@TEST\nA\n+\nI", true).unwrap();
        let result = fp.next().unwrap();
        assert!(result.is_err());
        let e = result.unwrap_err();
        assert!(e.error_type == ParseErrorType::InvalidRecord);
        assert!(e.msg == "Sequence and quality lengths differed");

        let mut fp = FastqParser::new(b"@test\nAGCT\n+\nIIIII\n@TEST\nA\n+\nI", true).unwrap();
        let result = fp.next().unwrap();
        assert!(result.is_err());
        let e = result.unwrap_err();
        assert!(e.error_type == ParseErrorType::InvalidRecord);
        assert!(e.msg == "Sequence and quality lengths differed");
    }

    #[test]
    fn test_fastq_across_buffer() {
        let test_seq = b"@A\nA\n+A\nA\n@B\nA\n+B\n!";
        let mut cursor = Cursor::new(&test_seq[9..]);
        // the buffer is aligned to the first record
        let mut rec_reader = RecBuffer::new(&mut cursor, test_seq[..9].to_vec()).unwrap();

        let used = {
            let mut rec_buffer = FastqParser::from_buffer(&rec_reader.buf, rec_reader.last);
            for _s in rec_buffer.by_ref() {
                // record is incomplete
                panic!("No initial record should be parsed")
            }
            rec_buffer.used()
        };

        // refill the buffer, but we're not done quite yet
        assert_eq!(rec_reader.refill(used).unwrap(), false);

        // now we should see both records
        let mut rec_buffer = FastqParser::from_buffer(&rec_reader.buf, rec_reader.last);

        // there should be a record assuming the parser
        // handled the buffer boundary
        let iterated_seq = rec_buffer.by_ref().next();
        let seq = iterated_seq.unwrap();
        assert_eq!(seq.unwrap().id, b"A");

        // but not another because the buffer's too short
        let iterated_seq = rec_buffer.by_ref().next();
        assert!(iterated_seq.is_none());

        // TODO: refill and check for the last record
    }

    #[test]
    fn test_fastq_with_random_tsv_inside() {
        let content = std::fs::read_to_string("./tests/data/random_tsv.fq").unwrap();
        let mut i = 0;
        let mut parser = FastqParser::new(content.as_bytes(), false).unwrap();
        loop {
            match parser.by_ref().next() {
                Some(Ok(_)) => i +=1,
                Some(Err(_)) => panic!("woops"),
                None => break,
            };
        };
        assert_eq!(i, 1);

        let mut j = 0;
        let res = parse_sequence_reader(content.as_bytes(), |_| {},|_| {
            j += 1;
        });
        assert!(res.is_err());
    }
}
