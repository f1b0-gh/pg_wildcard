// SPDX-License-Identifier: Apache-2.0
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::collections::BTreeMap;
use wildcard_puffin::{identity::{IdentityIndex,BLOB_TYPE,LEGACY_BLOB_TYPE},puffin::{Blob,Codec,PuffinWriter}};

fuzz_target!(|data: &[u8]| {
    if data.len()>1<<20 { return; }
    // Exercise hostile outer envelopes, then the uncompressed payload parser
    // behind an otherwise valid envelope so mutations reach dictionary logic.
    let _=IdentityIndex::read(data);
    let mut writer=PuffinWriter::new();
    let kind=if data.get(4)==Some(&1) { LEGACY_BLOB_TYPE } else { BLOB_TYPE };
    writer.add(Blob { blob_type:kind.into(),fields:vec![1,2],snapshot_id:42,sequence_number:3,
        properties:BTreeMap::from([
            ("table-uuid".into(),"00000000-0000-4000-8000-000000000001".into()),
            ("column".into(),"message".into()),("guard".into(),"event_id".into()),("guard-type".into(),"utf8".into())]),
        data:data.to_vec() },Codec::None).unwrap();
    let envelope=writer.finish(BTreeMap::new(),false).unwrap();
    if let Ok(index)=IdentityIndex::read(&envelope) {
        let encoded=index.write(42,3).unwrap();
        assert_eq!(IdentityIndex::read(&encoded).unwrap(),index);
    }
});
