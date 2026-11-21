// SPDX-License-Identifier: Apache-2.0
//! Allocation-free Thrift compact envelope validation. This does not replace
//! Parquet decoding: it only bounds counts, nesting, lengths and page sizes.
//! Field IDs follow Apache parquet-format's FileMetaData/PageHeader schema.
use super::super::LakeError;
use std::io::Read;

const MAX_NODES: usize = 100_000;
const MAX_DEPTH: usize = 32;

pub(super) struct Sizes {
    pub compressed: u64,
    pub uncompressed: u64,
}

pub(super) fn metadata(bytes: &[u8]) -> Result<(), LakeError> {
    let mut input = bytes;
    let mut parser = Parser {
        input: &mut input,
        nodes: MAX_NODES,
        metadata: true,
    };
    parser.structure(0, false)?;
    if !input.is_empty() {
        return Err(LakeError::Invalid("trailing Parquet metadata"));
    }
    Ok(())
}

pub(super) fn page(input: &mut impl Read) -> Result<Sizes, LakeError> {
    Parser {
        input,
        nodes: MAX_NODES,
        metadata: false,
    }
    .structure(0, true)?
    .ok_or(LakeError::Invalid("Parquet page size fields"))
}

struct Parser<'a, R> {
    input: &'a mut R,
    nodes: usize,
    metadata: bool,
}

impl<R: Read> Parser<'_, R> {
    fn byte(&mut self) -> Result<u8, LakeError> {
        let mut byte = [0];
        self.input.read_exact(&mut byte)?;
        Ok(byte[0])
    }
    fn unsigned(&mut self) -> Result<u64, LakeError> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            if shift == 63 && byte > 1 {
                return Err(LakeError::Invalid("compact integer overflow"));
            }
            value |= u64::from(byte & 127) << shift;
            if byte & 128 == 0 {
                return Ok(value);
            }
        }
        Err(LakeError::Invalid("compact integer overflow"))
    }
    fn signed(&mut self) -> Result<i64, LakeError> {
        let n = self.unsigned()?;
        Ok(((n >> 1) as i64) ^ -((n & 1) as i64))
    }
    fn charge(&mut self, n: usize) -> Result<(), LakeError> {
        self.nodes = self
            .nodes
            .checked_sub(n)
            .ok_or(LakeError::Limit("Parquet metadata/header nodes"))?;
        Ok(())
    }
    fn bytes(&mut self, mut n: u64) -> Result<(), LakeError> {
        let mut scratch = [0; 512];
        while n != 0 {
            let take = n.min(scratch.len() as u64) as usize;
            self.input.read_exact(&mut scratch[..take])?;
            n -= take as u64;
        }
        Ok(())
    }
    fn structure(&mut self, depth: usize, page: bool) -> Result<Option<Sizes>, LakeError> {
        if depth >= MAX_DEPTH {
            return Err(LakeError::Limit("Parquet metadata/header depth"));
        }
        let mut last = 0_i16;
        let (mut compressed, mut uncompressed) = (None, None);
        let mut page_type = None;
        let mut data = [false; 3];
        let mut levels = 0;
        loop {
            let header = self.byte()?;
            if header == 0 {
                break;
            }
            let kind = header & 15;
            let id = if header >> 4 == 0 {
                i16::try_from(self.signed()?).map_err(|_| LakeError::Invalid("compact field id"))?
            } else {
                last.checked_add(i16::from(header >> 4))
                    .ok_or(LakeError::Invalid("compact field id"))?
            };
            last = id;
            if self.metadata && depth == 0 && id == 2 {
                self.schema(kind, depth + 1)?;
            } else if page && id == 1 {
                self.charge(1)?;
                if kind != 5 {
                    return Err(LakeError::Invalid("Parquet page type"));
                }
                let value = self.signed()?;
                if !(0..=3).contains(&value) || page_type.replace(value).is_some() {
                    return Err(LakeError::Invalid("Parquet page type"));
                }
            } else if page && matches!(id, 2 | 3) {
                self.charge(1)?;
                if kind != 5 {
                    return Err(LakeError::Invalid("Parquet page size type"));
                }
                let value = self.signed()?;
                let value =
                    u64::try_from(value).map_err(|_| LakeError::Invalid("negative page size"))?;
                if value > i32::MAX as u64 {
                    return Err(LakeError::Invalid("Parquet page size"));
                }
                let target = if id == 2 {
                    &mut uncompressed
                } else {
                    &mut compressed
                };
                if target.replace(value).is_some() {
                    return Err(LakeError::Invalid("duplicate page size"));
                }
            } else if page && matches!(id, 5 | 7 | 8) {
                if kind != 12 {
                    return Err(LakeError::Invalid("Parquet data header type"));
                }
                let slot = match id {
                    5 => 0,
                    7 => 1,
                    _ => 2,
                };
                if data[slot] {
                    return Err(LakeError::Invalid("duplicate Parquet data header"));
                }
                data[slot] = true;
                let bytes = self.data_header(depth + 1, id)?;
                if id == 8 {
                    levels = bytes;
                }
            } else {
                self.value(kind, depth + 1, true)?;
            }
        }
        if page {
            let sizes = Sizes {
                compressed: compressed.ok_or(LakeError::Invalid("missing page size"))?,
                uncompressed: uncompressed.ok_or(LakeError::Invalid("missing page size"))?,
            };
            match page_type {
                Some(0) if data[0] => {}
                Some(1) => {}
                Some(2) if data[1] => {}
                Some(3) if data[2] => {}
                _ => return Err(LakeError::Invalid("missing Parquet data header")),
            }
            if levels > sizes.uncompressed || levels > sizes.compressed {
                return Err(LakeError::Invalid("Parquet level bytes exceed page"));
            }
            Ok(Some(sizes))
        } else {
            Ok(None)
        }
    }
    // Parquet encodes the logical schema tree as a flat list. Its scalar
    // num_children controls upstream reservation and recursion independently
    // of Thrift collection lengths and physical nesting.
    fn schema(&mut self, kind: u8, depth: usize) -> Result<(), LakeError> {
        if kind != 9 {
            return Err(LakeError::Invalid("Parquet schema list"));
        }
        let header = self.byte()?;
        if header & 15 != 12 {
            return Err(LakeError::Invalid("Parquet schema element"));
        }
        let count = if header >> 4 == 15 {
            self.unsigned()?
        } else {
            u64::from(header >> 4)
        };
        if count == 0 || count > 4096 {
            return Err(LakeError::Limit("Parquet schema elements"));
        }
        let mut remaining = [0_u64; MAX_DEPTH];
        let mut level = 0;
        for i in 0..count {
            self.charge(1)?;
            if i > 0 {
                while level > 0 && remaining[level - 1] == 0 {
                    level -= 1;
                }
                if level == 0 {
                    return Err(LakeError::Invalid("extra Parquet schema root"));
                }
                remaining[level - 1] -= 1;
            }
            let mut last = 0_i16;
            let mut children = None;
            loop {
                let field = self.byte()?;
                if field == 0 {
                    break;
                }
                let ty = field & 15;
                let id = if field >> 4 == 0 {
                    i16::try_from(self.signed()?)
                        .map_err(|_| LakeError::Invalid("compact field id"))?
                } else {
                    last.checked_add(i16::from(field >> 4))
                        .ok_or(LakeError::Invalid("compact field id"))?
                };
                last = id;
                if id == 2 {
                    self.charge(1)?;
                    if ty != 5 || !(1..=super::MAX_PAGE_BYTES as i64).contains(&self.signed()?) {
                        return Err(LakeError::Limit("Parquet fixed-length value width"));
                    }
                } else if id == 5 {
                    self.charge(1)?;
                    if ty != 5 {
                        return Err(LakeError::Invalid("Parquet schema children type"));
                    }
                    let n = u64::try_from(self.signed()?)
                        .map_err(|_| LakeError::Invalid("negative schema children"))?;
                    if n > count - i - 1 {
                        return Err(LakeError::Limit("Parquet schema child count"));
                    }
                    if children.replace(n).is_some() {
                        return Err(LakeError::Invalid("duplicate schema child count"));
                    }
                } else {
                    self.value(ty, depth + 1, true)?;
                }
            }
            if let Some(n) = children.filter(|n| *n > 0) {
                if level >= MAX_DEPTH {
                    return Err(LakeError::Limit("Parquet logical schema depth"));
                }
                remaining[level] = n;
                level += 1;
            }
        }
        if remaining[..level].iter().any(|n| *n != 0) {
            return Err(LakeError::Invalid("missing Parquet schema children"));
        }
        Ok(())
    }
    fn data_header(&mut self, depth: usize, header_kind: i16) -> Result<u64, LakeError> {
        if depth >= MAX_DEPTH {
            return Err(LakeError::Limit("Parquet metadata/header depth"));
        }
        let mut last = 0_i16;
        let v2 = header_kind == 8;
        let mut encoding = None;
        let mut counts = [None; 3];
        let mut lengths = [None; 2];
        loop {
            let header = self.byte()?;
            if header == 0 {
                break;
            }
            let kind = header & 15;
            let id = if header >> 4 == 0 {
                i16::try_from(self.signed()?).map_err(|_| LakeError::Invalid("compact field id"))?
            } else {
                last.checked_add(i16::from(header >> 4))
                    .ok_or(LakeError::Invalid("compact field id"))?
            };
            last = id;
            if id == if v2 { 4 } else { 2 } {
                self.charge(1)?;
                if kind != 5 {
                    return Err(LakeError::Invalid("Parquet encoding type"));
                }
                let value = self.signed()?;
                // Delta byte-array encodings may expand repeated prefixes or
                // allocate from internal counts before Arrow validates output.
                if !matches!(value, 0 | 2 | 8) || (header_kind == 7 && value != 0) {
                    return Err(LakeError::Limit(
                        "Parquet requires plain/dictionary page encoding",
                    ));
                }
                if encoding.replace(value).is_some() {
                    return Err(LakeError::Invalid("duplicate Parquet encoding"));
                }
            } else if id == 1 || (v2 && matches!(id, 2 | 3 | 5 | 6)) {
                self.charge(1)?;
                if kind != 5 {
                    return Err(LakeError::Invalid("Parquet data header number"));
                }
                let n = u64::try_from(self.signed()?)
                    .map_err(|_| LakeError::Invalid("negative Parquet data header number"))?;
                if id <= 3 {
                    if n > super::MAX_GROUP_ROWS as u64 {
                        return Err(LakeError::Limit("Parquet page values"));
                    }
                    if counts[(id - 1) as usize].replace(n).is_some() {
                        return Err(LakeError::Invalid("duplicate Parquet page count"));
                    }
                } else {
                    if n > super::MAX_PAGE_BYTES {
                        return Err(LakeError::Limit("Parquet page level bytes"));
                    }
                    if lengths[(id - 5) as usize].replace(n).is_some() {
                        return Err(LakeError::Invalid("duplicate Parquet page length"));
                    }
                }
            } else {
                self.value(kind, depth + 1, true)?;
            }
        }
        let values = counts[0].ok_or(LakeError::Invalid("missing Parquet page values"))?;
        if encoding.is_none() {
            return Err(LakeError::Invalid("missing Parquet page encoding"));
        }
        if v2 {
            if counts[1].is_none_or(|n| n > values) || counts[2].is_none_or(|n| n > values) {
                return Err(LakeError::Invalid("Parquet page rows/nulls"));
            }
            let definition =
                lengths[0].ok_or(LakeError::Invalid("missing Parquet level length"))?;
            let repetition =
                lengths[1].ok_or(LakeError::Invalid("missing Parquet level length"))?;
            return definition
                .checked_add(repetition)
                .ok_or(LakeError::Invalid("Parquet level size overflow"));
        }
        Ok(0)
    }
    fn value(&mut self, kind: u8, depth: usize, field: bool) -> Result<(), LakeError> {
        self.charge(1)?;
        if depth >= MAX_DEPTH {
            return Err(LakeError::Limit("Parquet metadata/header depth"));
        }
        match kind {
            1 | 2 if field => {}
            1 | 2 => {
                if !matches!(self.byte()?, 1 | 2) {
                    return Err(LakeError::Invalid("compact boolean"));
                }
            }
            3 => {
                self.byte()?;
            }
            4..=6 => {
                self.unsigned()?;
            }
            7 => self.bytes(8)?,
            8 => {
                let n = self.unsigned()?;
                self.bytes(n)?;
            }
            9 | 10 => {
                let header = self.byte()?;
                let n = if header >> 4 == 15 {
                    self.unsigned()?
                } else {
                    u64::from(header >> 4)
                };
                if n > self.nodes as u64 {
                    return Err(LakeError::Limit("Parquet collection count"));
                }
                for _ in 0..n {
                    self.value(header & 15, depth + 1, false)?;
                }
            }
            11 => {
                let n = self.unsigned()?;
                if n > (self.nodes / 2) as u64 {
                    return Err(LakeError::Limit("Parquet map count"));
                }
                if n > 0 {
                    let kinds = self.byte()?;
                    for _ in 0..n {
                        self.value(kinds >> 4, depth + 1, false)?;
                        self.value(kinds & 15, depth + 1, false)?;
                    }
                }
            }
            12 => {
                self.structure(depth, false)?;
            }
            _ => return Err(LakeError::Invalid("compact type")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_large_collection_before_visiting_elements() {
        assert!(matches!(
            metadata(&[0x19, 0xf5, 0xff, 0xff, 0xff, 0x7f]),
            Err(LakeError::Limit(_))
        ));
    }
    #[test]
    fn bounded_unknown_fields_and_truncation() {
        let valid = [0x11, 0x28, 3, b'a', b'b', b'c', 0x19, 0x25, 2, 4, 0];
        assert!(metadata(&valid).is_ok());
        for n in 0..valid.len() {
            assert!(metadata(&valid[..n]).is_err());
        }
        let mut deep = vec![0x1c; MAX_DEPTH + 1];
        deep.resize(deep.len() * 2, 0);
        assert!(matches!(metadata(&deep), Err(LakeError::Limit(_))));
    }
    #[test]
    fn logical_schema_counts_and_depth_are_bounded_before_upstream_parse() {
        // Metadata.schema=[SchemaElement { num_children=i32::MAX }].
        let count_attack = [0x29, 0x1c, 0x55, 0xfe, 0xff, 0xff, 0xff, 0x0f, 0, 0];
        assert!(matches!(
            metadata(&count_attack),
            Err(LakeError::Limit("Parquet schema child count"))
        ));
        let mut depth_attack = vec![0x29, 0xfc, 35];
        for _ in 0..34 {
            depth_attack.extend([0x55, 2, 0]);
        }
        depth_attack.extend([0, 0]);
        assert!(matches!(
            metadata(&depth_attack),
            Err(LakeError::Limit("Parquet logical schema depth"))
        ));
    }
    #[test]
    fn delta_encodings_and_v2_level_overflow_are_refused() {
        for encoding in [5, 6, 7] {
            let header = [
                0x15,
                0,
                0x15,
                0,
                0x15,
                0,
                0x2c,
                0x15,
                0,
                0x15,
                encoding << 1,
                0,
                0,
            ];
            assert!(matches!(
                page(&mut header.as_slice()),
                Err(LakeError::Limit(_))
            ));
        }
        let v2 = [
            0x15, 6, 0x15, 2, 0x15, 2, 0x5c, 0x15, 2, 0x15, 0, 0x15, 2, 0x15, 0, 0x15, 0xfe, 0xff,
            0xff, 0xff, 0x0f, 0, 0,
        ];
        assert!(matches!(
            page(&mut v2.as_slice()),
            Err(LakeError::Limit("Parquet page level bytes"))
        ));
    }
}
