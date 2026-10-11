package oracle;
public class JavaUse {
    public String use(BeanFactory factory) {
        return factory.getBeanProvider(String.class).getObject();
    }
}
