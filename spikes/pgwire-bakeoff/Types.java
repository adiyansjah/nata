import java.sql.*;
public class Types{
 public static void main(String[] a)throws Exception{
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/spike?sslmode=disable&preferQueryMode=simple&binaryTransfer=false","spike",""); Statement s=c.createStatement();ResultSet r=s.executeQuery("SELECT * FROM '/home/dev/tmp-4b/types.parquet'")){
   r.next();for(int i=1;i<=r.getMetaData().getColumnCount();i++){Object v=r.getObject(i);System.out.println(r.getMetaData().getColumnName(i)+" "+v.getClass().getSimpleName()+" "+v);}
  }
 }
}
