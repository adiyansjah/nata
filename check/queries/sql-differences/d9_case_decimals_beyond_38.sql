-- expect-error-nata: common decimal type
SELECT CASE WHEN f THEN dec3810 ELSE CAST(1 AS DECIMAL(38,0)) END AS a FROM '{data}/sqlcheck/types.parquet'
