SELECT l_returnflag, l_linestatus, l_extendedprice * (1 - l_discount) * (1 + l_tax) AS charge FROM '{data}/lineitem.parquet' WHERE l_shipdate <= DATE '1998-09-02'
