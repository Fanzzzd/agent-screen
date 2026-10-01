import Testing
@testable import Sidescreen

/// What dismissal does with a window by default.
struct DismissalTests {
    @Test func handoverAlwaysReturns() {
        for disposition in [nil, Disposition.keep, .close] {
            #expect(Dismissal.fate(origin: .handedOver, disposition: disposition, ofLaunchedApp: true) == .giveBack)
        }
    }

    @Test func dispositionDecidesTheAgentsWindows() {
        for origin in [WindowOrigin.launched, .placed, .found, nil] {
            #expect(Dismissal.fate(origin: origin, disposition: .keep, ofLaunchedApp: true) == .giveBack)
            #expect(Dismissal.fate(origin: origin, disposition: .close, ofLaunchedApp: false) == .close)
        }
    }

    @Test func withoutDispositionLaunchedClosesAndPlacedReturns() {
        #expect(Dismissal.fate(origin: .launched, disposition: nil, ofLaunchedApp: true) == .close)
        #expect(Dismissal.fate(origin: .placed, disposition: nil, ofLaunchedApp: true) == .giveBack)
    }

    @Test func untrackedWindowFollowsItsApp() {
        #expect(Dismissal.fate(origin: nil, disposition: nil, ofLaunchedApp: true) == .close)
        #expect(Dismissal.fate(origin: nil, disposition: nil, ofLaunchedApp: false) == .giveBack)
    }
}
