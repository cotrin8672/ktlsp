package annotationfixture;
import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@interface AtCode { String value(); String namespace(); }
@Retention(RetentionPolicy.CLASS)
@interface Invisible { String value(); }
enum Choice { FIRST, SECOND }
@interface Nested { String value(); }
@Retention(RetentionPolicy.RUNTIME)
@interface Payload {
    byte b(); char c(); double d(); float f(); int i(); long j(); short s(); boolean z();
    String text(); String utf16(); Choice choice(); Class<?> type(); Nested nested(); int[] array();
}
class InjectionPoint {}
@AtCode(value="CUSTOM",namespace="example")
@Invisible("classInvisible")
public class CustomPoint<T extends Number> extends InjectionPoint {
    @Invisible("fieldInvisible") public int field;
    @Payload(b=-7,c='\uD800',d=-0.0d,f=Float.POSITIVE_INFINITY,i=-17,j=9223372036854775807L,
        s=-123,z=true,text="nul\u0000\uD83D\uDE03",utf16="\uD800x\uDC00\uD83D\uDE03\u0000",choice=Choice.SECOND,type=int[][].class,
        nested=@Nested("nestedValue"),array={1,-2,3})
    @Invisible("methodInvisible") public void probe() {}
}
