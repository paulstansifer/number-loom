#clues

 For a puzzle of type "triddler" we expect to see six sets of clues, with types "top", "topright", "bottomright", "bottom", "bottomleft" and "topleft". This labeling assumes that the puzzle is oriented so that there are horizontal lines separating cells, but not vertical lines, and that the horizontal clues are at the left of the puzzle, like this:

[ASCII art omitted]     

The "topleft" and "bottomleft" clues are clues for horizontal rows of cells above and below the bend on the left side of the puzzle. The "top" and "topright" clues are for lines in the / direction. The "bottom" and "bottomright" clues are for lines in the \ direction. It is possible for some clue-sets to be empty (if the puzzle has a sharp corner).

The puzzle above would be represented like:

     <clues type="topleft">
     <line><count>1</count><count>1</count><count>1</count></line>
     <line><count>2</count><count>3</count></line>
     </clues>

     <clues type="bottomleft">
     <line><count>1</count></line>
     </clues>

     <clues type="top">
     <line><count>3</count></line>
     <line><count>2</count><count>1</count></line>
     </clues>

     <clues type="topright">
     <line><count>3</count></line>
     </clues>

     <clues type="bottom">
     <line><count>1</count></line>
     <line><count>2</count><count>1</count></line>
     </clues>

     <clues type="bottomright">
     <line><count>3</count></line>
     <line><count>2</count></line>
     </clues> 


-----

# solutions

 For grid type puzzles, the solution is given row-by-row. Each row starts and ends with a | character. There may be line-feeds separating the rows, but there need not be. The solution in the sample above could equally well be given as:

	<solution type="goal">
	<image>
	|.XX..||.XX.X||..X.X||.XXX.||X.X..||X.X..||..XX.||.X.X.||.X.XX||XX...|
	</image>
	</solution>
     

For triddlers the solution is also stored row-by-row, but the line starting and line ending characters are / or \ depending on the slope of the edge of the puzzle. Basically if the puzzle looks like this:

	      ________
	     /\ B/\D /\
	    /A_\/C_\/E_\
	   /\G /\I /\K /
	  /F_\/H_\/J_\/
	  \L /\ N/\ P/
	   \/_M\/_O\/ 

then we save it like this (except, of course, that the letters are replaced by whatever symbol indicates the color for that cell):

         /ABCDE\
        /FGHIJK/
        \LMNOP/ 