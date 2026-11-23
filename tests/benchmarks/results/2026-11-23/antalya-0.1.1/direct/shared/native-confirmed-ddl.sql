CREATE TABLE wc_benchmark_cd8eb66100624610b5dd2461dcd11076.bench
(
    `id` Int64,
    `v` String,
    INDEX v_byte_grams ngrams(concat(hex(substring(v, 1, 8192)), if(length(v) > 8192, '_wildcard_overflow_', '')), 6) TYPE text(tokenizer = 'array') GRANULARITY 100000000
)
ENGINE = MergeTree
ORDER BY id
SETTINGS index_granularity = 8192
