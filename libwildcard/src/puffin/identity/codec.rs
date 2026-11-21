// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::puffin::puffin::{Blob, Codec, PuffinWriter, read_blob_limited, read_footer};
const MAGIC: &[u8] = b"WCSI";
struct Writer(Vec<u8>);
impl Writer {
    fn bytes(&mut self, v: &[u8]) -> Result<(), Error> {
        ensure(v.len() <= MAX_BLOB_BYTES - self.0.len(), "blob budget")?;
        self.0.extend_from_slice(v);
        Ok(())
    }
    fn count(&mut self, n: usize) -> Result<(), Error> {
        self.bytes(
            &u32::try_from(n)
                .map_err(|_| Error::Invalid("count"))?
                .to_le_bytes(),
        )
    }
    fn string(&mut self, s: &str) -> Result<(), Error> {
        self.count(s.len())?;
        self.bytes(s.as_bytes())
    }
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let v = self.0.get(..n).ok_or(Error::Invalid("truncated payload"))?;
        self.0 = &self.0[n..];
        Ok(v)
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(
            self.bytes(4)?
                .try_into()
                .map_err(|_| Error::Invalid("u32"))?,
        ) as usize)
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(
            self.bytes(8)?
                .try_into()
                .map_err(|_| Error::Invalid("u64"))?,
        ))
    }
    fn count(&mut self, max: usize, min_bytes: usize) -> Result<usize, Error> {
        let n = self.u32()?;
        ensure(n <= max && n <= self.0.len() / min_bytes, "count budget")?;
        Ok(n)
    }
    fn string_ref(&mut self, max: usize) -> Result<&'a str, Error> {
        let n = self.count(max, 1)?;
        std::str::from_utf8(self.bytes(n)?).map_err(|_| Error::Invalid("UTF8"))
    }
    fn string(&mut self, max: usize) -> Result<String, Error> {
        Ok(self.string_ref(max)?.into())
    }
}
impl IdentityIndex {
    pub(super) fn encode_payload(&self) -> Result<Vec<u8>, Error> {
        self.encode_payload_version(2)
    }
    pub(super) fn encode_payload_version(&self, version: u8) -> Result<Vec<u8>, Error> {
        ensure(matches!(version, 1 | 2), "payload version")?;
        let mut w = Writer(Vec::new());
        w.bytes(MAGIC)?;
        w.bytes(&[version])?;
        let binding =
            serde_json::to_vec(&self.binding).map_err(|_| Error::Invalid("binding JSON"))?;
        ensure(binding.len() <= 1024, "binding budget")?;
        w.count(binding.len())?;
        w.bytes(&binding)?;
        w.count(self.max_value_bytes)?;
        w.count(self.views.len())?;
        for view in &self.views {
            w.bytes(&[*view as u8])?;
        }
        w.count(self.files.len())?;
        for f in &self.files {
            w.string(&f.path)?;
            w.bytes(&f.rows.to_le_bytes())?;
            w.bytes(&[u8::from(f.unrestricted)])?;
            let dictionary: BTreeMap<&str, usize> = if version == 2 {
                f.postings
                    .values()
                    .flat_map(|ids| ids.iter().map(AsRef::as_ref))
                    .collect::<BTreeSet<&str>>()
                    .into_iter()
                    .enumerate()
                    .map(|(ordinal, id)| (id, ordinal))
                    .collect()
            } else {
                BTreeMap::new()
            };
            if version == 2 {
                w.count(dictionary.len())?;
                for id in dictionary.keys() {
                    w.string(id)?;
                }
            }
            w.count(f.postings.len())?;
            for (&gram, ids) in &f.postings {
                w.bytes(&gram.to_le_bytes())?;
                w.count(ids.len())?;
                for id in ids {
                    if version == 1 {
                        w.string(id)?;
                    } else {
                        w.count(
                            *dictionary
                                .get(id.as_ref())
                                .ok_or(Error::Invalid("dictionary reference"))?,
                        )?;
                    }
                }
            }
        }
        Ok(w.0)
    }
    pub(super) fn decode_payload(data: &[u8]) -> Result<Self, Error> {
        ensure(data.len() <= MAX_BLOB_BYTES, "blob budget")?;
        let mut r = Reader(data);
        ensure(r.bytes(MAGIC.len())? == MAGIC, "magic/version")?;
        let version = r.byte()?;
        ensure(matches!(version, 1 | 2), "payload version")?;
        let n = r.count(1024, 1)?;
        let binding: Binding =
            serde_json::from_slice(r.bytes(n)?).map_err(|_| Error::Invalid("binding JSON"))?;
        binding.validate()?;
        let max_value_bytes = r.u32()?;
        ensure(
            (1..=MAX_VALUE_BYTES).contains(&max_value_bytes),
            "value budget",
        )?;
        let n = r.count(View::ALL.len(), 1)?;
        ensure(n > 0, "empty views")?;
        let mut views = Vec::new();
        for _ in 0..n {
            let v = View::from_u8(r.byte()?).ok_or(Error::Invalid("view"))?;
            ensure(views.last().is_none_or(|old| *old < v), "view order")?;
            views.push(v);
        }
        let n = r.count(MAX_FILES, 17)?;
        let mut files = Vec::new();
        let mut paths = BTreeSet::new();
        let mut path_bytes = 0;
        let mut used = Usage::default();
        let mut dictionary_bytes = 0usize;
        for _ in 0..n {
            let path = r.string(MAX_PATH_BYTES - path_bytes)?;
            path_bytes += path.len();
            ensure(
                !path.is_empty() && paths.insert(path.clone()),
                "duplicate/empty path",
            )?;
            let rows = r.u64()?;
            let flag = r.byte()?;
            ensure(flag <= 1, "fallback flag")?;
            let unrestricted = flag == 1;
            let mut dictionary: Vec<Arc<str>> = Vec::new();
            if version == 2 {
                let count = r.count(MAX_POSTINGS - used.postings, 4)?;
                ensure(!unrestricted || count == 0, "fallback dictionary")?;
                for _ in 0..count {
                    let id = r.string_ref(MAX_ID_BYTES.min(MAX_GUARD_BYTES - dictionary_bytes))?;
                    dictionary_bytes += id.len();
                    ensure(
                        dictionary.last().is_none_or(|old| old.as_ref() < id),
                        "dictionary order",
                    )?;
                    dictionary.push(Arc::from(id));
                }
            }
            // Legacy strings are also interned within the current bounded file.
            let mut legacy_dictionary: BTreeMap<&str, Arc<str>> = BTreeMap::new();
            let count = r.count(MAX_GRAMS - used.grams, 16)?;
            ensure(!unrestricted || count == 0, "fallback postings")?;
            used.grams += count;
            let mut postings = BTreeMap::new();
            let mut previous = None;
            for _ in 0..count {
                let gram = r.u64()?;
                let (view, symbols) = crate::core::grams::unpack(gram);
                let view = view.ok_or(Error::Invalid("gram view"))?;
                ensure(
                    views.contains(&view)
                        && symbols.iter().all(|s| *s <= crate::core::grams::END)
                        && crate::core::grams::key(view, symbols[0], symbols[1], symbols[2])
                            == gram,
                    "gram encoding",
                )?;
                ensure(previous.is_none_or(|g| g < gram), "gram order")?;
                previous = Some(gram);
                let count = r.count(IDS_PER_GRAM.min(MAX_POSTINGS - used.postings), 4)?;
                ensure(count > 0, "empty posting")?;
                used.postings += count;
                let mut ids = BTreeSet::new();
                let mut prior: Option<Arc<str>> = None;
                for _ in 0..count {
                    let id = if version == 1 {
                        let text = r.string_ref(MAX_ID_BYTES.min(MAX_GUARD_BYTES - used.bytes))?;
                        legacy_dictionary
                            .entry(text)
                            .or_insert_with(|| Arc::from(text))
                            .clone()
                    } else {
                        let ordinal = r.u32()?;
                        dictionary
                            .get(ordinal)
                            .ok_or(Error::Invalid("dictionary reference"))?
                            .clone()
                    };
                    ensure(
                        id.len() <= MAX_GUARD_BYTES - used.bytes,
                        "guard byte budget",
                    )?;
                    used.bytes += id.len();
                    ensure(prior.as_ref().is_none_or(|old| old < &id), "guard order")?;
                    prior = Some(id.clone());
                    ids.insert(id);
                }
                postings.insert(gram, ids);
            }
            files.push(FileIndex {
                path,
                rows,
                unrestricted,
                postings,
            });
        }
        ensure(r.0.is_empty(), "trailing payload")?;
        let result = Self {
            binding,
            views,
            max_value_bytes,
            files,
        };
        // Reject alternate JSON encodings/duplicate fields as well as noncanonical wire forms.
        ensure(
            result.encode_payload_version(version)? == data,
            "noncanonical payload",
        )?;
        Ok(result)
    }
    pub fn write(&self, snapshot_id: i64, sequence_number: i64) -> Result<Vec<u8>, Error> {
        let b = &self.binding;
        let mut w = PuffinWriter::new();
        w.add(
            Blob {
                blob_type: BLOB_TYPE.into(),
                fields: vec![b.column_field_id, b.guard_field_id],
                snapshot_id,
                sequence_number,
                properties: BTreeMap::from([
                    ("table-uuid".into(), b.table_uuid.clone()),
                    ("column".into(), b.column.clone()),
                    ("guard".into(), b.guard.clone()),
                    ("guard-type".into(), "utf8".into()),
                ]),
                data: self.encode_payload()?,
            },
            Codec::Zstd,
        )?;
        let file = w.finish(
            BTreeMap::from([("created-by".into(), "pg_wildcard".into())]),
            false,
        )?;
        ensure(file.len() <= MAX_FILE_BYTES, "Puffin budget")?;
        Ok(file)
    }
    pub fn read(file: &[u8]) -> Result<Self, Error> {
        ensure(file.len() <= MAX_FILE_BYTES, "Puffin budget")?;
        let footer = read_footer(file)?;
        ensure(footer.blobs.len() == 1, "expected own single-blob Puffin")?;
        let blob = &footer.blobs[0];
        let version = match blob.blob_type.as_str() {
            BLOB_TYPE => 2,
            LEGACY_BLOB_TYPE => 1,
            _ => return Err(Error::Invalid("blob type/version")),
        };
        let payload = read_blob_limited(file, blob, MAX_BLOB_BYTES)?;
        ensure(payload.get(4) == Some(&version), "footer/payload version")?;
        let index = Self::decode_payload(&payload)?;
        let b = &index.binding;
        ensure(
            blob.fields == [b.column_field_id, b.guard_field_id]
                && blob.properties.get("table-uuid") == Some(&b.table_uuid)
                && blob.properties.get("column") == Some(&b.column)
                && blob.properties.get("guard") == Some(&b.guard)
                && blob
                    .properties
                    .get("guard-type")
                    .is_some_and(|t| t == "utf8"),
            "footer binding",
        )?;
        Ok(index)
    }
}
