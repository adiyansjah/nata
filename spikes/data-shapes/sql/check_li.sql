WITH f AS (SELECT * FROM read_parquet('@DIR@/li_@NAME@.parquet')), n AS (SELECT count(*)::DOUBLE AS n FROM f)
SELECT 'rows', count(*), @NL@, @NL@ FROM f
UNION ALL SELECT 'null_share_l_quantity', avg((l_quantity IS NULL)::INT), @P@ - 4 * sqrt(@P@ * (1 - @P@) / (SELECT n FROM n)), @P@ + 4 * sqrt(@P@ * (1 - @P@) / (SELECT n FROM n)) FROM f
UNION ALL SELECT 'null_share_l_discount', avg((l_discount IS NULL)::INT), @P@ - 4 * sqrt(@P@ * (1 - @P@) / (SELECT n FROM n)), @P@ + 4 * sqrt(@P@ * (1 - @P@) / (SELECT n FROM n)) FROM f
UNION ALL SELECT 'null_share_l_shipdate (P/2)', avg((l_shipdate IS NULL)::INT), @P@ / 2 - 4 * sqrt(@P@ / 2 * (1 - @P@ / 2) / (SELECT n FROM n)), @P@ / 2 + 4 * sqrt(@P@ / 2 * (1 - @P@ / 2) / (SELECT n FROM n)) FROM f
UNION ALL SELECT 'null_share_l_returnflag (PK)', avg((l_returnflag IS NULL)::INT), @PK@ - 4 * sqrt(@PK@ * (1 - @PK@) / (SELECT n FROM n)), @PK@ + 4 * sqrt(@PK@ * (1 - @PK@) / (SELECT n FROM n)) FROM f
UNION ALL SELECT 'distinct_returnflag_non_null', count(DISTINCT l_returnflag), 3, 3 FROM f
UNION ALL SELECT 'q6_discount_in_range_rows (rows reach the sum when P is small)', count(*) FILTER (WHERE l_discount BETWEEN 0.05 AND 0.07), 0, @NL@ FROM f;
