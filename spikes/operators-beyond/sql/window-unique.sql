select o_orderkey, o_custkey, o_totalprice, o_orderstatus,
  row_number() over w as rn, lag(o_totalprice) over w as prev, sum(o_totalprice) over (partition by o_custkey) as tot
from orders window w as (partition by o_custkey order by o_totalprice desc, o_orderkey)
