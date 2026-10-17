package genericfixture;

public class Lexical<T extends U, U> {
    public <U extends Other> T lexical() { return null; }
}
