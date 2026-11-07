import java.sql.*;
import java.time.LocalDate;
public class DateDomains {
 public static void main(String[] a) throws Exception {
  try (Connection n=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata","test","");
       Connection p=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[1]+"/nata","test","")) {
   System.out.println("pgjdbc "+n.getMetaData().getDriverVersion());
   for(int d:new int[]{-2440588,-735160,-719528,-719162,0,2932896,2932897,2145042905}) {
    try(PreparedStatement ns=n.prepareStatement("SELECT d FROM '"+a[2]+"/d"+d+".parquet'");
        PreparedStatement ps=p.prepareStatement("SELECT '1970-01-01'::date + ("+d+")")) {
     for(int run=0;run<10;run++) {
      try(ResultSet nr=ns.executeQuery();ResultSet pr=ps.executeQuery()) {
       nr.next();pr.next();LocalDate actual=nr.getObject(1,LocalDate.class),expected=pr.getObject(1,LocalDate.class);
       if(!actual.equals(expected) || !nr.getString(1).equals(pr.getString(1)) || !nr.getObject(1).equals(pr.getObject(1)))throw new AssertionError(d+" "+actual+" "+expected);
       if(run==9)System.out.println(d+" LocalDate="+actual+" default="+nr.getObject(1)+" text="+nr.getString(1));
      }
     }
    }
   }
   for(int d:new int[]{Integer.MIN_VALUE,-2147483647,-2440589,2145042906,Integer.MAX_VALUE}) {
    try(Statement s=n.createStatement()){s.executeQuery("SELECT d FROM '"+a[2]+"/d"+d+".parquet'");throw new AssertionError("range accepted");}
    catch(SQLException e){if(!"22008".equals(e.getSQLState()))throw e;}
   }
   System.out.println("PASS default preparation, text/binary, Date/LocalDate, range errors");
  }
 }
}
