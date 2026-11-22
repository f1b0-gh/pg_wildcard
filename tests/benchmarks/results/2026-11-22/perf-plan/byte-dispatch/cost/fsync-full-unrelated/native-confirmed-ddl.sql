CREATE TABLE wc_benchmark_2bda1b73fb2341e2b262d6fa585245b2.bench
(
    `id` Int64,
    `v` String,
    INDEX v_byte_grams ngrams(concat(hex(substring(v, 1, 8192)), if(length(v) > 8192, '_wildcard_overflow_', '')), 6) TYPE text(tokenizer = 'array') GRANULARITY 100000000
)
ENGINE = MergeTree
ORDER BY id
SETTINGS fsync_after_insert = 1, fsync_part_directory = 1, index_granularity = 8192
