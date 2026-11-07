SELECT true AS k, count(*) AS n FROM '{data}/sqlcheck/types.parquet' GROUP BY true;
