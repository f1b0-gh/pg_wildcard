CREATE TABLE wc_benchmark_dc2d6ca7224a4eada30f1924608f3bf0.bench
(
    `id` Int64,
    `v` String,
    INDEX v_byte_grams ngrams(concat(hex(substring(v, 1, 8192)), if(length(v) > 8192, '_wildcard_overflow_', '')), 6) TYPE text(tokenizer = 'array') GRANULARITY 100000000
)
ENGINE = MergeTree
ORDER BY id
SETTINGS fsync_after_insert = 1, fsync_part_directory = 1, index_granularity = 8192
