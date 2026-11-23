CREATE TABLE wc_benchmark_d05bd4c4559e4c3b840a3ca5dc048b86.bench
(
    `id` Int64,
    `v` String,
    INDEX v_byte_grams ngrams(concat(hex(substring(v, 1, 8192)), if(length(v) > 8192, '_wildcard_overflow_', '')), 6) TYPE text(tokenizer = 'array') GRANULARITY 100000000
)
ENGINE = MergeTree
ORDER BY id
SETTINGS index_granularity = 8192
