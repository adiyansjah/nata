select o_orderpriority, l_shipmode, count(*), sum(l_extendedprice)
from orders join lineitem on o_orderkey = l_orderkey
group by o_orderpriority, l_shipmode;
