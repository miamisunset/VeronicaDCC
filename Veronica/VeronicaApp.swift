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
    var body: some Scene {
        WindowGroup {
            ContentView(
                store: Store(initialState: AppFeature.State()) {
                    AppFeature()
                }
            )
        }
    }
}
