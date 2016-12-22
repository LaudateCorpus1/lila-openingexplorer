package lila.openingexplorer

sealed abstract class SpeedGroup(
    val id: Int,
    val name: String,
    val range: Range) {
}

object SpeedGroup {

  case object Bullet extends SpeedGroup(1, "bullet", 0 to 179)
  case object Blitz extends SpeedGroup(2, "blitz", 180 to 479)
  case object Classical extends SpeedGroup(3, "classical", 480 to Int.MaxValue)

  val all = List(Bullet, Blitz, Classical)

  val byId = all map { v => (v.id, v) } toMap

  def apply(speed: chess.Speed) = speed match {
    case chess.Speed.Bullet                                 => Bullet
    case chess.Speed.Blitz                                  => Blitz
    case chess.Speed.Classical | chess.Speed.Correspondence => Classical
  }

  def apply(limit: Int, increment: Int): SpeedGroup =
    SpeedGroup(chess.Speed(chess.Clock(limit, increment)))

  private val timeControlRegex = """(\d+)\+(\d+)""".r

  def fromTimeControl(timeControl: String): SpeedGroup = timeControl match {
    case timeControlRegex(l, i) =>
      val speed = for {
        limit <- parseIntOption(l)
        increment <- parseIntOption(i)
      } yield SpeedGroup(limit, increment)

      speed.getOrElse(Classical)

    case _ => Classical
  }
}
