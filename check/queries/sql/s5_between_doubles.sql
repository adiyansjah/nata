SELECT id, d BETWEEN CAST(0 AS DOUBLE) AND CAST('NaN' AS DOUBLE) AS a FROM '{data}/sqlcheck/types.parquet'
