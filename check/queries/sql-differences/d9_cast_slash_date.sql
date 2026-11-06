-- expect-error-nata: not a valid DATE
SELECT CAST('2024/02/29' AS DATE) AS a FROM '{data}/sqlcheck/types.parquet'
