import java.sql.*;
import java.nio.file.*;
import java.io.*;
import java.math.BigDecimal;
public class AggregateClients {
 static String csv(Object v) {
  if(v==null)return "NULL";
  String s=v instanceof BigDecimal ? ((BigDecimal)v).toPlainString() : v.toString();
  return "\""+s.replace("\"","\"\"")+"\"";
 }
 public static void main(String[] a)throws Exception {
  try(Connection c=DriverManager.getConnection("jdbc:postgresql://127.0.0.1:"+a[0]+"/nata","test","")) {
   System.out.println("pgjdbc="+c.getMetaData().getDriverVersion()+" java="+System.getProperty("java.version"));
   for(Path p:Files.list(Path.of(a[1])).sorted().toList()) {
    try(PreparedStatement s=c.prepareStatement(Files.readString(p))) {
     for(int repeat=0;repeat<10;repeat++) {
      try(ResultSet r=s.executeQuery();PrintWriter out=new PrintWriter(Path.of(a[2],p.getFileName()+".csv").toFile())) {
       int n=r.getMetaData().getColumnCount();
       while(r.next()) {
        for(int i=1;i<=n;i++) {
         Object v=r.getObject(i);int t=r.getMetaData().getColumnType(i);
         if(v!=null && t==Types.BIGINT && !(v instanceof Long))throw new AssertionError("BIGINT native type");
         if(v!=null && t==Types.NUMERIC && !(v instanceof BigDecimal))throw new AssertionError("numeric native type");
         out.print((i>1?",":"")+csv(v));
        }
        out.println();
       }
      }
     }
    }
   }
  }
 }
}
