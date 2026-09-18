//
//  VeronicaApp.swift
//  Veronica
//
//  Created by Alex Yermolaev on 9/15/26.
//

import ComposableArchitecture
import SwiftUI

@main
struct VeronicaApp: App {
    /// Shared root store: windows and the Panes menu send into the same
    /// feature state, so menu items and toolbar buttons stay in sync.
    let store: StoreOf<AppFeature>

    init() {
        // UI-test geometry pin (issue #94): clear before any window exists,
        // while restoration can still be preempted. No-op without the flag.
        if WindowLaunchOptions.hasFlag {
            WindowLaunchOptions.clearRestoredFrames()
        }
        store = Store(initialState: AppFeature.State()) {
            AppFeature()
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView(store: store)
        }
        .commands {
            // Static titles only: commands do not observe the store, so a
            // title read from state would freeze at its initial value while
            // the toolbar sibling (via `@Bindable`) keeps updating. Each
            // item names its explicit target state instead.
            CommandMenu("Panes") {
                Button("Swap Graph and Parameters") {
                    store.send(.nodeGraph(.paneOrderToggled))
                }
                Button("Stack Graph and Parameters Vertically") {
                    store.send(.nodeGraph(.paneOrientationChanged(.column)))
                }
                Button("Place Graph and Parameters Side by Side") {
                    store.send(.nodeGraph(.paneOrientationChanged(.row)))
                }
            }
        }
    }
}
