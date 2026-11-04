SELECT 'hot_build_rows', count(*), @B@, @B@ FROM read_parquet('@DIR@/hot_build.parquet')
UNION ALL SELECT 'hot_build_share_of_key_0', avg((bk = 0)::INT), 0.30 - 4 * sqrt(0.21 / count(*)), 0.30 + 4 * sqrt(0.21 / count(*)) FROM read_parquet('@DIR@/hot_build.parquet')
UNION ALL SELECT 'hot_probe_rows_with_key_0 (exactly 100)', count(*), 100, 100 FROM read_parquet('@DIR@/hot_probe.parquet') WHERE pk = 0
UNION ALL SELECT 'hh_a_share_of_key_0', avg((ak = 0)::INT), 0.30 - 4 * sqrt(0.21 / count(*)), 0.30 + 4 * sqrt(0.21 / count(*)) FROM read_parquet('@DIR@/hh_a.parquet')
UNION ALL SELECT 'hh_b_share_of_key_0', avg((bk2 = 0)::INT), 0.30 - 4 * sqrt(0.21 / count(*)), 0.30 + 4 * sqrt(0.21 / count(*)) FROM read_parquet('@DIR@/hh_b.parquet')
UNION ALL SELECT 'hh_join_rows_over_(0.3 HH)^2', (SELECT count(*) FROM read_parquet('@DIR@/hh_a.parquet') a JOIN read_parquet('@DIR@/hh_b.parquet') b ON a.ak = b.bk2)::DOUBLE / ((0.3 * @HH@) * (0.3 * @HH@)), 0.7, 3.0;
