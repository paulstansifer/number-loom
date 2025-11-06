use axohtml::{html, text};

use crate::puzzle::{Clue, Puzzle};

pub fn as_html<C: Clue>(puzzle: &Puzzle<C>) -> String {
    let html: axohtml::dom::DOMTree<String> = html!(
        <html>
            <head>
            <title></title>
            <style>
            {text!(
"
table, td, th {
    border-collapse: collapse;
}
td {
    border: 1px solid black;
    width: 40px;
    height: 40px;
}

table tr:nth-of-type(5n) td {
    border-bottom: 3px solid;
}
table td:nth-of-type(5n) {
    border-right: 3px solid;
}

table tr:last-child td {
    border-bottom: 1px solid;
}
table td:last-child {
    border-right: 1px solid;
}
.col {
  vertical-align: bottom;
  border-top: none;
  font-family: courier;
}
.row {
  text-align: right;
  border-left: none;
  font-family: courier;
  padding-right: 6px;
}


    ")}
            </style>
            </head>
            <body>
                <table>
                    <thead>
                        <tr>
                        <th></th>
                        { puzzle.cols.iter().map(|col| html!(<th class="col">{
                            col.iter().map(|clue| html!(<div style=(clue.html_color(puzzle))>{text!("{} ", clue.html_text(puzzle))} </div>))
                        }</th>))}
                        </tr>
                    </thead>
                    <tbody>
                    {
                        puzzle.rows.iter().map(|row| html!(<tr><th class="row">{
                            row.iter().map(|clue| html!(<span style=(clue.html_color(puzzle))>{text!("{} ", clue.html_text(puzzle))} </span>))
                        }</th>
                        {
                            puzzle.cols.iter().map(|_| html!(<td></td>))
                        }
                        </tr>))
                    }
                    </tbody>
                </table>
            </body>
        </html>
    );

    html.to_string()
}
