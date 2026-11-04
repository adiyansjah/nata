-- NULL share of every column against the requested probability @P@ (exact for 0 and 1)
WITH f AS (SELECT * FROM read_parquet('@DIR@/nul_p@PP@.parquet')), n AS (SELECT count(*)::DOUBLE AS n FROM f),
 t AS (SELECT 4 * sqrt(@P@ * (1 - @P@) / (SELECT n FROM n)) AS tol)
SELECT 'rows', count(*), @NP@, @NP@ FROM f
UNION ALL SELECT 'null_share_kn', avg((kn IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'null_share_ki', avg((ki IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'null_share_kd', avg((kd IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'null_share_ks', avg((ks IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'null_share_v', avg((v IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'null_share_w', avg((w IS NULL)::INT), @P@ - (SELECT tol FROM t), @P@ + (SELECT tol FROM t) FROM f
UNION ALL SELECT 'empty_string_share_of_non_null_ks (0.05; none if all NULL)', coalesce(avg((ks = '')::INT) FILTER (WHERE ks IS NOT NULL), 0.05), 0.0, 0.15 FROM f
UNION ALL SELECT 'nul_dimn_null_key_rows', count(*), @K10@, @K10@ FROM read_parquet('@DIR@/nul_dimn.parquet') WHERE dk IS NULL
UNION ALL SELECT 'nul_sdim_rows_with_empty_and_null_key', count(*), 10, 10 FROM read_parquet('@DIR@/nul_sdim.parquet') WHERE dks IS NULL OR dks = '';
