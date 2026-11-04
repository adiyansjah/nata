select o_comment, count(*), sum(o_totalprice)
from orders
where o_orderdate <= date '1998-09-02'
group by o_comment;
