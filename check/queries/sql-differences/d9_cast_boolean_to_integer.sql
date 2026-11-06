-- expect-error-nata: conversions between booleans and numbers
SELECT CAST(f AS INTEGER) AS a FROM '{data}/sqlcheck/types.parquet'
