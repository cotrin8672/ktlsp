package protectedfixture;

public class ProtectedApi {
    protected float getBreakSpeed() { return 1.0f; }
    protected Position transform(Position value) { return value; }
    public int known() { return 2; }
    private int hiddenControl() { return 3; }
}
