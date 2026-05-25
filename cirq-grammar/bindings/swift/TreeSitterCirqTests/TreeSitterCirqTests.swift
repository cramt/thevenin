import XCTest
import SwiftTreeSitter
import TreeSitterCirq

final class TreeSitterCirqTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_cirq())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading Cirq grammar")
    }
}
