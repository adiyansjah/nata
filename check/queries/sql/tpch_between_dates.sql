SELECT l_orderkey FROM '{data}/lineitem.parquet' WHERE l_shipdate BETWEEN DATE '1995-03-01' AND DATE '1995-03-31' AND l_discount NOT BETWEEN 0.02 AND 0.08
