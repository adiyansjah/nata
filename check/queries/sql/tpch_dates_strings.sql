SELECT l_orderkey, CAST(l_shipdate AS VARCHAR) AS d, CAST(l_quantity AS INTEGER) AS q FROM '{data}/lineitem.parquet' WHERE l_orderkey < 40
