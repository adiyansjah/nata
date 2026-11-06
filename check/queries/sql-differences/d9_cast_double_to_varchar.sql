-- expect-error-nata: text form of floating point
SELECT CAST(d AS VARCHAR) AS a FROM '{data}/sqlcheck/types.parquet'
