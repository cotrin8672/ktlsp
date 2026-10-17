package fixture;

public class ShapeTarget {
    public enum Axis { X, Y }
    public record Point(int value) {}
    public Axis axis(Axis value) { return value; }
    public Point point(Point value) { return value; }
    public Object any(Object value) { return value; }
    public String text(String value) { return value; }
}
