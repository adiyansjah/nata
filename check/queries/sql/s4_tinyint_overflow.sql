-- expect-error
SELECT ti + ti AS a FROM '{data}/sqlcheck/types.parquet'
