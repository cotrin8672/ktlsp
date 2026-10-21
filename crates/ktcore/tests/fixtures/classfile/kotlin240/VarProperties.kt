package settercontrol
open class Box<T>(open var value: T)
class TextBox(override var value: String) : Box<String>(value)
class Flags(var isReady: Boolean, var is_foo: String, var is0: Int, var isfoo: String)
class Body { var text: String = "body"
 var hidden: String = "hidden"
  private set
}
