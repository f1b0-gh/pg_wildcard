SELECT version() AS version,
       match('a', '\0') AS literal_nul_against_a,
       match(unhex('610062'), '\0') AS literal_nul_against_nul_value,
       match('a', '\\x00') AS escaped_nul_against_a,
       match(unhex('610062'), '\\x00') AS escaped_nul_against_nul_value
FORMAT JSONEachRow
