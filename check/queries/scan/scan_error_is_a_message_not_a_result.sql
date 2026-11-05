-- expect-error-nata: no column named
-- scan: {data}/nation.parquet --columns n_nosuchcolumn
SELECT n_nosuchcolumn FROM '{data}/nation.parquet'
