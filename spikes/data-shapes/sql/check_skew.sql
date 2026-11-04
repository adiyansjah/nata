-- name|value|lo|hi ; every value must lie in [lo, hi]. Shares have a 4-sigma tolerance (tiny N gives a wide one).
WITH f AS (SELECT * FROM read_parquet('@DIR@/skew_fact.parquet')), n AS (SELECT count(*)::DOUBLE AS n FROM f)
SELECT 'fact_rows', count(*), @N@, @N@ FROM f
UNION ALL SELECT 'khot_share_of_key_0', avg((khot = 0)::INT), 0.30 - 4 * sqrt(0.21 / (SELECT n FROM n)), 0.30 + 4 * sqrt(0.21 / (SELECT n FROM n)) FROM f
UNION ALL SELECT 'k15_share_of_rank_1 (Zipf 1.5: about 29%)', avg((k15 = 1)::INT), 0.20, 0.38 FROM f
UNION ALL SELECT 'k10_share_of_rank_1 (Zipf 1.0: about ln2/ln K)', avg((k10 = 1)::INT), 0.6 * ln(2) / ln(@K@), 1.6 * ln(2) / ln(@K@) FROM f
UNION ALL SELECT 'k10_share_of_top_100_ranks', avg((k10 <= 100)::INT), 0.5 * ln(101) / ln(@K@), 1.5 * ln(101) / ln(@K@) FROM f
UNION ALL SELECT 'k15_max_key_le_K', max(k15), 1, @K@ FROM f
UNION ALL SELECT 'k10_distinct_over_K', count(DISTINCT k10) / @K@::DOUBLE, 0.02, 1.0 FROM f
UNION ALL SELECT 'kuni_distinct_over_min(K,rows)', count(DISTINCT kuni) / least(@K@, count(*))::DOUBLE, 0.5, 1.0 FROM f
UNION ALL SELECT 'shipmode_distinct', count(DISTINCT l_shipmode), 5, 20 FROM f
UNION ALL SELECT 'dim_rows', count(*), @K@ + 1, @K@ + 1 FROM read_parquet('@DIR@/skew_dim.parquet');
