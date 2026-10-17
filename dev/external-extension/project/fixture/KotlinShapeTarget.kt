package fixture

class KotlinShapeTarget {
    enum class Axis { X, Y }
    object Marker
    fun axis(value: Axis): Axis = value
    fun marker(value: Marker): Marker = value
}
