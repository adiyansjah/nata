SELECT count(*) AS n, count(i) AS c, sum(i) AS s, min(s) AS lo, max(dt) AS hi FROM '{data}/sqlcheck/all_null.parquet';
