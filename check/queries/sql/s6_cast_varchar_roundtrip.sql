SELECT id, CAST(CAST(dec52 AS VARCHAR) AS DECIMAL(5,2)) AS a, CAST(CAST(dt AS VARCHAR) AS DATE) AS b FROM '{data}/sqlcheck/types.parquet'
