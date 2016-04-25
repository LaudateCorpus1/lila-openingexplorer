package lila.openingexplorer

import scala.collection.mutable.WrappedArray

import scalaz.Validation.FlatMap._

object Util {

  def situationMoves(situation: chess.Situation): List[chess.Move] = {
    // deduplicate castling moves
    situation.moves.values.flatten.foldLeft(List.empty[chess.Move] -> Set.empty[chess.Pos]) {
      case ((list, seenRooks), move) => move.castle match {
        case Some((_, (rookPos, _))) =>
          if (seenRooks(rookPos)) (list, seenRooks)
          else (move :: list, seenRooks + rookPos)
        case _ => (move :: list, seenRooks)
      }
    }._1.flatMap { move =>
      move :: (
        if (move.promotes)
          // expand underpromotions
          List(
            move.withPromotion(chess.Knight.some),
            move.withPromotion(chess.Bishop.some),
            move.withPromotion(chess.Rook.some)
          ).flatten
        else Nil
      )
    }
  }

  def situationDrops(situation: chess.Situation): List[chess.Drop] = {
    val droppablePositions = situation.drops.getOrElse(chess.Pos.all filterNot situation.board.pieces.contains)
    (for {
      role <- situation.board.crazyData.map(_.pockets(situation.color).roles.distinct).getOrElse(List.empty)
      pos <- droppablePositions
    } yield situation.drop(role, pos).toOption).flatten
  }

  def situationMovesOrDrops(situation: chess.Situation): List[chess.MoveOrDrop] =
    situationMoves(situation).map(Left(_)) ::: situationDrops(situation).map(Right(_))

  def distinctHashes(hashes: List[chess.PositionHash]): Array[chess.PositionHash] =
    hashes.map(h => (h: WrappedArray[Byte])).distinct.map(_.array).toArray

  def wrapLog[A](before: String, after: String)(f: => A): A = {
    val start = System.currentTimeMillis
    println(before)
    val res = f
    val duration = System.currentTimeMillis - start
    println(f"$after (${duration / 1000d}%.02f seconds)")
    res
  }
}
