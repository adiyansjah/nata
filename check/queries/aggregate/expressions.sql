SELECT i % 3 AS k, count(*) AS n, sum(CAST(ti AS BIGINT) + 1) AS s FROM '{data}/sqlcheck/types.parquet' GROUP BY i % 3;
