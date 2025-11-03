// SPDX-License-Identifier: AGPL-3.0-or-later
//
// Copyright (C) 2025 A1 XYZ, INC.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anchor_lang::declare_program;

declare_program!(solayer);
declare_program!(endoavs);
// TODO: this doesn't work because Anchor 0.30.1 doesn't like SmallVec or Vec
// I would like us to leave this here so we can try it with 0.31.1,
// else we can contribute a fix upstream.
// declare_program!(squads);
