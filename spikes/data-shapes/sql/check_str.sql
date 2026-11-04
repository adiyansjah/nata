WITH f AS (SELECT * FROM read_parquet('@DIR@/str_fact.parquet'))
SELECT 'fact_rows', count(*), @M@, @M@ FROM f
UNION ALL SELECT 's_hi_distinct', count(DISTINCT s_hi), 0.7 * @MH@ * (1 - exp(-@M@ / @MH@::DOUBLE)), @MH@ FROM f
UNION ALL SELECT 's_hi_length', min(length(s_hi)), 20, 20 FROM f
UNION ALL SELECT 's_long_min_length', min(length(s_long)), 100, 500 FROM f
UNION ALL SELECT 's_long_max_length', max(length(s_long)), 450, 500 FROM f
UNION ALL SELECT 's_long_distinct', count(DISTINCT s_long), 0.6 * @ML@ * (1 - exp(-@M@ / @ML@::DOUBLE)), @ML@ FROM f
UNION ALL SELECT 's_empty_share_empty', sum((s_empty = '')::INT) / count(*)::DOUBLE, 0.40 - 4 * sqrt(0.24 / count(*)), 0.40 + 4 * sqrt(0.24 / count(*)) FROM f
UNION ALL SELECT 's_empty_share_null', avg((s_empty IS NULL)::INT), 0.03 - 4 * sqrt(0.03 / count(*)), 0.03 + 4 * sqrt(0.03 / count(*)) FROM f
UNION ALL SELECT 's_utf8_share_non_ascii (every row)', avg((strlen(s_utf8) > length(s_utf8))::INT), 1, 1 FROM f
UNION ALL SELECT 's_utf8_share_with_combining_acute', avg(contains(s_utf8, chr(769))::INT), 0.08, 0.20 FROM f
UNION ALL SELECT 's_utf8_share_with_4_byte_chars', avg((contains(s_utf8, chr(128578)) OR contains(s_utf8, chr(120088)))::INT), 0.05, 0.30 FROM f
UNION ALL SELECT 's_last_distinct_prefixes', count(DISTINCT left(s_last, length(s_last) - 1)), 0.9 * least(2000, @M@), 2000 FROM f
UNION ALL SELECT 's_last_distinct_over_prefixes (differ only in the last byte)', count(DISTINCT s_last) / count(DISTINCT left(s_last, length(s_last) - 1))::DOUBLE, 0.5 * least(62, @M@ / 2000.0), 62 FROM f
UNION ALL SELECT 's_first_distinct_over_tails (differ only in the first byte)', count(DISTINCT s_first) / count(DISTINCT substr(s_first, 2))::DOUBLE, 0.5 * least(62, @M@ / 2000.0), 62 FROM f
UNION ALL SELECT 's_len_distinct', count(DISTINCT s_len), 0.9 * least(300, @M@), 300 FROM f
UNION ALL SELECT 'shipinstruct_distinct', count(DISTINCT l_shipinstruct), 0.5 * @MI@, @MI@ FROM f
UNION ALL SELECT 'str_dim_rows', count(*), @ML@, @ML@ FROM read_parquet('@DIR@/str_dim.parquet')
UNION ALL SELECT 'str_dim2_rows', count(*), @MH@, @MH@ FROM read_parquet('@DIR@/str_dim2.parquet')
UNION ALL SELECT 'str_dim3_rows', count(*), @MI@ * 9 // 10, @MI@ * 9 // 10 FROM read_parquet('@DIR@/str_dim3.parquet')
UNION ALL SELECT 'str_dim4_rows_unique', count(*) - count(DISTINCT dku), 0, 0 FROM read_parquet('@DIR@/str_dim4.parquet')
UNION ALL SELECT 'str_small_empty_rows', count(*), 20, 60 FROM read_parquet('@DIR@/str_small.parquet') WHERE sk = '';
