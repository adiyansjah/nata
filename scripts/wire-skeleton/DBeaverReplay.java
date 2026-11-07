import java.sql.*;
import java.nio.file.*;
public class DBeaverReplay {
 public static void main(String[] args)throws Exception {
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+args[0]+"/nata?user=nata")) {
   System.out.println("driver="+c.getMetaData().getDriverVersion());
   for(int n=1;n<args.length;n++) {
    try(Statement s=c.createStatement();ResultSet r=s.executeQuery(Files.readString(Path.of(args[n])))) {
     int rows=0;while(r.next()){for(int j=1;j<=r.getMetaData().getColumnCount();j++)r.getObject(j);rows++;}
     System.out.println((n-1)+": PASS rows="+rows);
    }catch(SQLException e){System.out.println((n-1)+": FAIL "+e.getSQLState());}
   }
  }
 }
}
