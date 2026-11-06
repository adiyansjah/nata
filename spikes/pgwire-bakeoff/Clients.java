import java.sql.*;
public class Clients {
 public static void main(String[] args)throws Exception {
  String mode=args[1];String q="SELECT v FROM '/home/dev/tmp-4b/million.parquet'";
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/spike?sslmode=disable&preferQueryMode="+mode+"&prepareThreshold=0&binaryTransfer=false","spike","")){
   try(PreparedStatement s=c.prepareStatement(q+" LIMIT 1")){try(ResultSet r=s.executeQuery()){if(!r.next()||!r.getString(1).equals("0")||r.next())throw new Exception("wrong row");}System.out.println(mode+" query PASS");}
   try(Statement s=c.createStatement()){
    Thread cancel=new Thread(()->{try{Thread.sleep(100);s.cancel();}catch(Exception e){throw new RuntimeException(e);}});cancel.start();
    try(ResultSet r=s.executeQuery(q)){int n=0;while(r.next())n++;System.out.println(mode+" cancel MISS rows="+n);}catch(SQLException e){System.out.println(mode+" cancel SQLSTATE="+e.getSQLState());}cancel.join();
   }
  }
 }
}
