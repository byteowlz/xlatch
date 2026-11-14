import XCTest

@MainActor final class GestureTests: XCTestCase {
    private func launch() -> XCUIApplication {
        continueAfterFailure = false
        let app = XCUIApplication(); app.launch()
        XCTAssertTrue(app.staticTexts["Project 30 · Send to session"].waitForExistence(timeout: 10))
        return app
    }
    private func swipe(_ app: XCUIApplication, row: Int, dx: CGFloat) {
        let text = app.staticTexts["Project \(row) · Send to session"]
        XCTAssertTrue(text.waitForExistence(timeout: 5))
        // The pinned Send bar reduces the viewport after the first selection.
        // Coordinate drags do not auto-scroll offscreen elements as taps do.
        let send = app.buttons["share-multi-send"]
        if !text.isHittable || (send.exists && text.frame.maxY >= send.frame.minY - 36) { app.swipeUp() }
        XCTAssertTrue(text.isHittable)
        let start = text.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: start.withOffset(CGVector(dx: dx, dy: 0)))
    }
    func testTransferStatusDoesNotMoveTargets() {
        let app = XCUIApplication()
        app.launchArguments = ["--top", "--activity-test"]
        app.launch()
        let row = app.staticTexts["Project 1 · Send to session"]
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        let original = row.frame
        let next = app.buttons["fixture-next-activity"]
        next.tap()
        XCTAssertTrue(app.staticTexts["share-activity-label"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.textFields["share-target-search"].isHittable, "Search stays available during transfers")
        let dock = app.descendants(matching: .any).matching(identifier: "share-activity-status").firstMatch
        XCTAssertGreaterThan(dock.frame.minY, app.frame.midY, "Progress belongs below the list")
        XCTAssertEqual(row.frame.minY, original.minY, accuracy: 1)
        XCTAssertEqual(row.frame.width, original.width, accuracy: 1)
        next.tap()
        XCTAssertTrue(app.staticTexts["Sending 1 of 2"].waitForExistence(timeout: 3))
        XCTAssertEqual(row.frame.minY, original.minY, accuracy: 1)
        XCTAssertEqual(row.frame.height, original.height, accuracy: 1)
        let capture = XCTAttachment(screenshot: app.screenshot())
        capture.name = "Bottom transfer dock does not move targets"; capture.lifetime = .keepAlways; add(capture)
        next.tap()
        XCTAssertTrue(app.textFields["share-target-search"].waitForExistence(timeout: 3))
        XCTAssertEqual(row.frame.minY, original.minY, accuracy: 1)
    }
    func testShortLeftSwipeSelectsWithoutRevealingAnActionButton() {
        let app = launch()
        swipe(app, row: 30, dx: -72)
        XCTAssertTrue(app.buttons["share-multi-send"].waitForExistence(timeout: 3), app.debugDescription)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 1 target"), app.buttons["share-multi-send"].label)
        XCTAssertFalse(app.buttons["Select"].exists)
        let disabled = XCTAttachment(screenshot: app.screenshot()); disabled.name = "One selected — green Send"; disabled.lifetime = .keepAlways; add(disabled)
        swipe(app, row: 29, dx: -72)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 2 targets"))
        XCTAssertTrue(app.buttons["share-multi-send"].isEnabled)
        XCTAssertTrue(app.staticTexts["Project 22 · Send to session"].isHittable, "Selection must not leave a blank gap above the remaining rows")
        let enabled = XCTAttachment(screenshot: app.screenshot()); enabled.name = "Two selected — enabled Send"; enabled.lifetime = .keepAlways; add(enabled)
        swipe(app, row: 29, dx: -72)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 1 target"))
        XCTAssertTrue(app.buttons["share-multi-send"].isEnabled)
    }
    func testEmptySelectionHasReadableDisabledSend() {
        let app = launch()
        app.buttons["Select multiple"].tap()
        let send = app.buttons["share-multi-send"]
        XCTAssertTrue(send.waitForExistence(timeout: 3))
        XCTAssertFalse(send.isEnabled)
        let capture = XCTAttachment(screenshot: app.screenshot()); capture.name = "No selection — neutral disabled Send"; capture.lifetime = .keepAlways; add(capture)
        swipe(app, row: 29, dx: -72)
        XCTAssertTrue(send.label.contains("Send to 1 target"))
        XCTAssertTrue(send.isEnabled)
    }
    func testShortRightSwipeStartsChain() {
        let app = launch()
        swipe(app, row: 30, dx: 72)
        XCTAssertTrue(app.staticTexts["Your chain"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["share-multi-send"].exists)
    }
    func testVerticalScrollingDoesNotSelect() {
        let app = launch()
        app.swipeDown()
        XCTAssertFalse(app.buttons["share-multi-send"].exists)
        XCTAssertFalse(app.staticTexts["Project 30 · Send to session"].isHittable)
    }
    func testSearchKeepsSelectedTargetsAndSupportsTypos() {
        let app = launch()
        swipe(app, row: 30, dx: -72)
        swipe(app, row: 29, dx: -72)
        let search = app.textFields["share-target-search"]
        search.tap(); search.typeText("prj30")
        XCTAssertTrue(app.staticTexts["Project 30 · Send to session"].exists)
        XCTAssertFalse(app.staticTexts["Project 29 · Send to session"].exists)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 2 targets"))
        app.buttons["Clear target search"].tap()
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 2 targets"))
        // Rows outside the viewport are virtualized after clearing the query.
        search.tap(); search.typeText("prj29")
        XCTAssertTrue(app.staticTexts["Project 29 · Send to session"].exists)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 2 targets"))
    }
    func testNoteRemainsEditableDuringFuzzySearch() {
        let app = XCUIApplication()
        app.launchArguments = ["--top"]
        app.launch()
        let search = app.textFields["share-target-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 10))
        search.tap(); search.typeText("prj30")
        let note = app.descendants(matching: .any).matching(identifier: "share-note-input").firstMatch
        XCTAssertTrue(note.isHittable)
        note.tap(); note.typeText("Summarize this")
        XCTAssertEqual(note.value as? String, "Summarize this")
        XCTAssertEqual(search.value as? String, "prj30")
        XCTAssertTrue(app.staticTexts["Project 30 · Send to session"].exists)
        app.buttons["Clear target search"].tap()
        XCTAssertEqual(note.value as? String, "Summarize this")
    }
    func testTileSwipeSelectsAndSurvivesLayoutChange() {
        let app = XCUIApplication()
        app.launchArguments = ["--top", "--tiles"]
        app.launch()
        let search = app.textFields["share-target-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 10))
        search.tap(); search.typeText("prj30")
        app.keyboards.buttons["Search"].tap()
        swipe(app, row: 30, dx: -72)
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 1 target"))
        app.buttons["share-target-layout"].tap()
        XCTAssertTrue(app.buttons["share-multi-send"].label.contains("Send to 1 target"))
        swipe(app, row: 30, dx: -72)
        XCTAssertFalse(app.buttons["share-multi-send"].isEnabled)
    }
    func testTileLayoutAndSearchInDarkLightAndLargeType() {
        for arguments in [["--top", "--tiles"], ["--top", "--tiles", "--light"], ["--top", "--tiles", "--large-type"]] {
            let app = XCUIApplication()
            app.launchArguments = arguments
            app.launch()
            let search = app.textFields["share-target-search"]
            XCTAssertTrue(search.waitForExistence(timeout: 10))
            search.tap(); search.typeText("prj1")
            XCTAssertTrue(app.staticTexts["Project 1 · Send to session"].waitForExistence(timeout: 3))
            app.keyboards.buttons["Search"].tap()
            let capture = XCTAttachment(screenshot: app.screenshot())
            capture.name = "Target tiles " + arguments.joined(separator: " ")
            capture.lifetime = .keepAlways; add(capture)
            app.buttons["share-target-layout"].tap()
            XCTAssertEqual(app.buttons["share-target-layout"].label, "Show tiles")
            app.terminate()
        }
    }

}
